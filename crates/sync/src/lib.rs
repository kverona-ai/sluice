//! Desktop ↔ mobile sync channel (02 §5.4 / §5.7, 05 §7.1): one-time QR pairing,
//! LAN-direct-first transport with an end-to-end encrypted relay fallback, the
//! review-queue protocol, signed 放行 / 驳回 and the audit trail of who decided.
//!
//! Shared by the desktop (`server`), the phone shells through `sluice-ffi`
//! (`client`) and the `sluice pair` / `sluice remote` CLI stand-in.

pub mod client;
pub mod crypto;
pub mod link;
pub mod pairing;
pub mod proto;
pub mod relay;
pub mod server;
pub mod store;
pub mod transport;

pub use client::{Cache, Client, ConnInfo, Decided};
pub use pairing::PairingPayload;
pub use proto::*;
pub use server::{Backend, DecisionOutcome, DecisionRequest, SessionInfo, Status, SyncServer};

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;

    struct FakeBackend {
        decisions: Mutex<Vec<DecisionRequest>>,
        changed: AtomicBool,
    }

    impl Backend for FakeBackend {
        fn repo_view(&self) -> Option<RepoView> {
            Some(RepoView {
                name: "sample".into(),
                branch: "main".into(),
                head_short: "abc1234".into(),
                vcs: "git".into(),
                ..Default::default()
            })
        }
        fn queue(&self) -> Vec<ReviewItem> {
            vec![ReviewItem {
                id: 7,
                client: "claude-code".into(),
                kind: "commit".into(),
                title: "提交 feat: x".into(),
                version: "v1".into(),
                state: "pending".into(),
                ..Default::default()
            }]
        }
        fn decide(&self, req: DecisionRequest) -> DecisionOutcome {
            let expired = req.version != "v1";
            self.decisions.lock().unwrap().push(req);
            if expired {
                DecisionOutcome {
                    outcome: "expired".into(),
                    detail: "baseline moved".into(),
                }
            } else {
                DecisionOutcome {
                    outcome: "done".into(),
                    detail: "committed abc1234".into(),
                }
            }
        }
        fn log(&self, offset: u32, _limit: u32) -> (u32, Vec<LogRow>) {
            (
                1,
                if offset == 0 {
                    vec![LogRow {
                        oid: "abc".into(),
                        short: "abc".into(),
                        subject: "init".into(),
                        ..Default::default()
                    }]
                } else {
                    vec![]
                },
            )
        }
        fn commit(&self, oid: &str) -> Option<CommitDetail> {
            (oid == "abc").then(|| CommitDetail {
                oid: "abc".into(),
                subject: "init".into(),
                ..Default::default()
            })
        }
        fn diff(&self, _oid: &str, path: &str) -> anyhow::Result<(String, bool)> {
            Ok((
                format!("--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n-a\n+b\n"),
                false,
            ))
        }
        fn on_devices_changed(&self) {
            self.changed.store(true, Ordering::Relaxed);
        }
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("sluice-sync-{name}-{}", crypto::random_id("")));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn pair_decide_and_resume_over_lan() {
        let desk_dir = tmp("desk");
        let dev_dir = tmp("dev");
        let backend = Arc::new(FakeBackend {
            decisions: Mutex::new(Vec::new()),
            changed: AtomicBool::new(false),
        });
        let server = SyncServer::start(&desk_dir, backend.clone(), "test").unwrap();
        let mut payload = server.begin_pairing().unwrap();
        // Force loopback so the test does not depend on the host's LAN setup.
        let port = server.status().lan_port.unwrap();
        payload.lan = vec![format!("127.0.0.1:{port}")];
        let text = payload.encode();

        let client = Client::new(&dev_dir, "test", "0");
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev2 = events.clone();
        client.set_sink(Some(Arc::new(move |e| ev2.lock().unwrap().push(e))));
        let info = client.pair(&text).unwrap();
        assert_eq!(info.via, "lan");
        assert_eq!(info.desktop_id, server.desktop_id());
        assert_eq!(server.devices().len(), 1);
        assert!(
            server.status().pairing.is_none(),
            "one-time code must be consumed"
        );

        // initial snapshot arrived with the welcome
        let c = client.cache();
        assert_eq!(c.repo.as_ref().unwrap().name, "sample");
        assert_eq!(c.queue.len(), 1);
        // read model calls
        let (total, rows) = client.log(0, 10).unwrap();
        assert_eq!((total, rows.len()), (1, 1));
        assert_eq!(client.commit("abc").unwrap().subject, "init");
        assert!(client.diff("abc", "README.md").unwrap().0.contains("+b"));
        // signed decision
        let d = client.decide(7, "v1", true, "ok from the phone").unwrap();
        assert_eq!(d.outcome, "done");
        let req = backend.decisions.lock().unwrap()[0].clone();
        assert_eq!(req.device.id, client.device_id());
        assert!(req.accept);
        // stale version → expired
        let d = client.decide(7, "v0", true, "").unwrap();
        assert_eq!(d.outcome, "expired");
        // broadcast reaches the device
        server.broadcast_event(DomainEvent::QueueChanged { pending: 0 });
        let t0 = std::time::Instant::now();
        while t0.elapsed() < Duration::from_secs(5) {
            if events
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, DomainEvent::QueueChanged { pending: 0 }))
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            events
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, DomainEvent::QueueChanged { .. }))
        );
        // resume with the device key (no pairing window open)
        client.disconnect();
        let t0 = std::time::Instant::now();
        while server.connected_count() > 0 && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let info = client.connect(None).unwrap();
        assert_eq!(info.via, "lan");
        assert_eq!(client.refresh().unwrap().queue.len(), 1);
        // revoke → the device can no longer resume
        client.disconnect();
        assert!(server.revoke(&client.device_id()));
        std::thread::sleep(Duration::from_millis(100));
        assert!(client.connect(None).is_err());
        server.shutdown();
        let _ = std::fs::remove_dir_all(desk_dir);
        let _ = std::fs::remove_dir_all(dev_dir);
    }

    #[test]
    fn relay_fallback_carries_a_session() {
        let desk_dir = tmp("desk-relay");
        let dev_dir = tmp("dev-relay");
        let stop = Arc::new(AtomicBool::new(false));
        let relay_addr = relay::serve("127.0.0.1:0", stop.clone()).unwrap();
        let backend = Arc::new(FakeBackend {
            decisions: Mutex::new(Vec::new()),
            changed: AtomicBool::new(false),
        });
        let server = SyncServer::start(&desk_dir, backend, "test").unwrap();
        server.set_relay(Some(relay_addr.to_string()));
        let t0 = std::time::Instant::now();
        while !server.status().relay_connected && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            server.status().relay_connected,
            "desktop should park on the relay"
        );
        let mut payload = server.begin_pairing().unwrap();
        payload.lan = vec!["127.0.0.1:9".into()]; // unreachable → forces the relay path
        let client = Client::new(&dev_dir, "test", "0");
        let info = client.pair(&payload.encode()).unwrap();
        assert_eq!(info.via, "relay");
        assert_eq!(client.cache().queue.len(), 1);
        let d = client.decide(7, "v1", false, "nope").unwrap();
        assert_eq!(d.outcome, "done");
        client.disconnect();
        stop.store(true, Ordering::Relaxed);
        server.shutdown();
        let _ = std::fs::remove_dir_all(desk_dir);
        let _ = std::fs::remove_dir_all(dev_dir);
    }
}
