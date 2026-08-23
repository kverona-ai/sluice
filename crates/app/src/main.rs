//! `sluice` — desktop app entry point and the CLI that the AI tool adapters,
//! git helpers and `open` command share (05 §2). Subcommands that belong to
//! later milestones are registered now (so the absolute-path contract in the
//! adapters is stable) and report their milestone until implemented.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use gpui::{
    App, AppContext as _, Application, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions,
    WindowBackgroundAppearance, WindowBounds, WindowOptions, actions, point, px, size,
};
use gpui_component::Root;
use sluice_domain::Repo;
use sluice_ui::Workbench;

#[derive(Parser, Debug)]
#[command(
    name = "sluice",
    version,
    about = "Sluice — the IDEA-grade Git workbench for AI coding agents"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Repository path (same as `sluice open <path>`).
    path: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Open a repository in the desktop app (default when a path is given).
    Open { path: Option<PathBuf> },
    /// Print the log as text (debug aid for the gix read path).
    Log {
        path: Option<PathBuf>,
        #[arg(long, default_value_t = 30)]
        limit: usize,
        /// `git rev-list --topo-order` instead of `--date-order`.
        #[arg(long)]
        topo: bool,
    },
    /// Serve the built-in read-only MCP server over stdio (03 §2).
    Mcp {
        #[command(subcommand)]
        cmd: McpCommand,
    },
    /// AI tool hookup: detect installed AI CLIs and register Sluice as their MCP server (03 §2).
    Ai {
        #[command(subcommand)]
        cmd: AiCommand,
    },
    /// Receive an AI tool hook event (JSON on stdin, or as the last argument for
    /// Codex `notify`) and record it for session provenance (03 §4). Always exits 0.
    Hook { tool: String, payload: Option<String> },
    /// GIT_ASKPASS / SSH_ASKPASS helper (M2, 05 §3).
    Askpass { prompt: Option<String> },
    /// GIT_EDITOR helper for non-interactive reword / squash (M3, 05 §6).
    Editor { file: Option<PathBuf> },
    /// GIT_SEQUENCE_EDITOR helper for interactive rebase (M3, 05 §6).
    SeqEditor { file: Option<PathBuf> },
    /// Export a diagnostics bundle (05 §9.2).
    Diagnostics,
}

#[derive(Subcommand, Debug)]
enum AiCommand {
    /// Show which AI CLIs are installed and whether Sluice is registered in each.
    Status,
    /// Register Sluice as an MCP server (all installed tools, or one with --tool).
    Connect {
        #[arg(long)]
        tool: Option<String>,
    },
    /// Remove the Sluice MCP entry from one tool's configuration.
    Disconnect {
        #[arg(long)]
        tool: String,
    },
}

#[derive(Subcommand, Debug)]
enum McpCommand {
    Serve {
        /// Repository to serve (default: current directory).
        #[arg(long)]
        repo: Option<PathBuf>,
    },
}

actions!(sluice, [Quit]);

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    match cli.command {
        None => run_app(cli.path),
        Some(Command::Open { path }) => run_app(path.or(cli.path)),
        Some(Command::Log { path, limit, topo }) => dump_log(path.or(cli.path), limit, topo),
        Some(Command::Mcp {
            cmd: McpCommand::Serve { repo },
        }) => {
            let path = resolve_path(repo.or(cli.path))?;
            sluice_bridge::mcp::McpServer::new(path).serve()
        }
        Some(Command::Ai { cmd }) => run_ai(cmd),
        Some(Command::Hook { tool, payload }) => {
            let raw = match payload {
                Some(p) => p,
                None => {
                    let mut buf = String::new();
                    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf);
                    buf
                }
            };
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.trim()) {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let mut ev = sluice_bridge::provenance::normalize(&tool, &v, now);
                if ev.cwd.is_empty() {
                    ev.cwd = std::env::current_dir()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();
                }
                if let Err(e) = sluice_bridge::provenance::record(&ev) {
                    tracing::warn!("provenance record failed: {e:#}");
                }
            }
            Ok(())
        }
        Some(Command::Askpass { prompt }) => {
            let repo = std::env::var_os("SLUICE_REPO")
                .map(PathBuf::from)
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from("."));
            match sluice_bridge::ipc::askpass(&repo, prompt.as_deref().unwrap_or("Credential"))? {
                Some(secret) => {
                    println!("{secret}");
                    Ok(())
                }
                None => std::process::exit(1),
            }
        }
        Some(Command::Editor { .. }) => not_yet("sluice editor", "M3 — 05 §6"),
        Some(Command::SeqEditor { .. }) => not_yet("sluice seq-editor", "M3 — 05 §6"),
        Some(Command::Diagnostics) => {
            let path = resolve_path(cli.path).unwrap_or_else(|_| PathBuf::from("."));
            let mut out = String::new();
            out.push_str(&format!("sluice {}\n", env!("CARGO_PKG_VERSION")));
            out.push_str(&format!(
                "os: {} {}\n",
                std::env::consts::OS,
                std::env::consts::ARCH
            ));
            let git = sluice_backend_cli::env::find_git();
            out.push_str(&format!(
                "git: {}\n",
                git.as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "NOT FOUND".into())
            ));
            if let Some(g) = &git
                && let Ok(o) = std::process::Command::new(g).arg("--version").output()
            {
                out.push_str(&format!("git version: {}", String::from_utf8_lossy(&o.stdout)));
            }
            out.push_str(&format!(
                "login PATH: {}\n",
                sluice_backend_cli::env::login_path()
            ));
            out.push_str(&format!("repo: {}\n", path.display()));
            out.push_str(&format!(
                "desktop ipc reachable: {}\n",
                sluice_bridge::ipc::ping(&path)
            ));
            for st in sluice_bridge::connect::status_all() {
                out.push_str(&format!("ai {:<12} {:?}\n", st.id, st.state));
            }
            let file = std::env::temp_dir().join(format!("sluice-diagnostics-{}.txt", std::process::id()));
            std::fs::write(&file, &out)?;
            print!("{out}");
            println!("written to {}", file.display());
            Ok(())
        }
    }
}

fn run_ai(cmd: AiCommand) -> Result<()> {
    use sluice_bridge::connect::{self, Registration};
    match cmd {
        AiCommand::Status => {
            let (c, a) = connect::server_command();
            println!("server command: {c} {}", a.join(" "));
            for st in connect::status_all() {
                let state = match &st.state {
                    Registration::NotInstalled => "not installed".to_string(),
                    Registration::Installed => "installed, not registered".to_string(),
                    Registration::Registered(cmd) => format!("registered -> {cmd}"),
                };
                println!("{:<14} {:<28} {}", st.id, state, st.config_path.display());
            }
            Ok(())
        }
        AiCommand::Connect { tool } => {
            let targets: Vec<&connect::ToolSpec> = connect::TOOLS
                .iter()
                .filter(|t| tool.as_deref().is_none_or(|id| id == t.id))
                .filter(|t| tool.is_some() || connect::status(t).state == Registration::Installed)
                .collect();
            if targets.is_empty() {
                println!("nothing to do (use `sluice ai status`)");
                return Ok(());
            }
            for spec in targets {
                match connect::register(spec) {
                    Ok(r) => println!(
                        "ok   {} via {}{}{}",
                        r.tool,
                        r.method,
                        r.backup
                            .map(|b| format!(" (backup {})", b.display()))
                            .unwrap_or_default(),
                        match r.verified {
                            Some(true) => ", verified",
                            Some(false) => ", NOT verified",
                            None => "",
                        }
                    ),
                    Err(e) => println!("fail {} — {e:#}", spec.name),
                }
            }
            Ok(())
        }
        AiCommand::Disconnect { tool } => {
            let spec = connect::TOOLS
                .iter()
                .find(|t| t.id == tool)
                .ok_or_else(|| anyhow::anyhow!("unknown tool {tool}"))?;
            connect::unregister(spec)?;
            println!("removed sluice from {}", spec.name);
            Ok(())
        }
    }
}

fn not_yet(what: &str, when: &str) -> Result<()> {
    eprintln!("{what}: not implemented yet ({when}). See sluice-doc requirements v0.3.");
    std::process::exit(2);
}

fn resolve_path(path: Option<PathBuf>) -> Result<PathBuf> {
    let p = match path {
        Some(p) => p,
        None => std::env::current_dir().context("cannot read current directory")?,
    };
    Ok(p)
}

fn dump_log(path: Option<PathBuf>, limit: usize, topo: bool) -> Result<()> {
    use sluice_core::{GitReader, LogOrder, LogQuery};
    let path = resolve_path(path)?;
    let reader = sluice_backend_gix::GixReader::discover(&path, sluice_core::Console::new())?;
    let info = reader.info()?;
    println!(
        "{} ({}) HEAD={} upstream={:?} ahead={} behind={}",
        info.name,
        info.git_dir.display(),
        info.head.branch.as_deref().unwrap_or("detached"),
        info.head.upstream,
        info.head.ahead,
        info.head.behind
    );
    let mut q = LogQuery {
        limit,
        ..Default::default()
    };
    if topo {
        q.order = LogOrder::TopoOrder;
    }
    let commits = reader.log(&q)?;
    let layout = sluice_graph::layout(commits.iter().map(|c| sluice_graph::Node {
        id: &c.id,
        parents: &c.parents,
        tip_ref: None,
    }));
    for (c, row) in commits.iter().zip(layout.rows.iter()) {
        let mut lanes = vec![' '; layout.max_lanes as usize * 2];
        for e in &row.out_edges {
            lanes[(e.from_lane as usize) * 2] = '|';
        }
        lanes[(row.lane as usize) * 2] = '*';
        println!(
            "{} {} {:>2} {:<16} {}",
            lanes.iter().collect::<String>(),
            c.id.short(8),
            c.agent.mark(),
            c.author.name.chars().take(16).collect::<String>(),
            c.summary
        );
    }
    Ok(())
}

fn run_app(path: Option<PathBuf>) -> Result<()> {
    let path = resolve_path(path)?;
    if let Ok(exe) = std::env::current_exe() {
        // SAFETY: set before any thread is spawned; read later by GitCli::command.
        unsafe { std::env::set_var("SLUICE_ASKPASS_EXE", exe) };
    }
    let repo = Repo::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let (ipc_tx, ipc_rx) = async_channel::unbounded();
    let ipc_repo = repo
        .cli
        .as_ref()
        .map(|c| c.workdir().to_path_buf())
        .unwrap_or_else(|| path.clone());
    match sluice_bridge::ipc::serve(ipc_repo.clone(), ipc_tx) {
        Ok(ep) => tracing::info!("ipc listening on 127.0.0.1:{} for {}", ep.port, ep.repo.display()),
        Err(e) => tracing::warn!("ipc server not started: {e:#}"),
    }
    let title = format!(
        "{} — {}",
        repo.info.name,
        repo.info.head.branch.as_deref().unwrap_or("detached HEAD")
    );

    Application::new()
        .with_assets(sluice_ui::assets::Assets)
        .run(move |cx: &mut App| {
            sluice_ui::init(cx);
            cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
            cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
            cx.set_menus(vec![Menu {
                name: "Sluice".into(),
                items: vec![MenuItem::action("Quit Sluice", Quit)],
            }]);
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let quit_repo = ipc_repo.clone();
            cx.on_app_quit(move |_| {
                let repo = quit_repo.clone();
                async move { sluice_bridge::ipc::remove_endpoint(&repo) }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(1400.), px(860.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(title.clone().into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(12.), px(13.))),
                }),
                window_min_size: Some(size(px(1100.), px(680.))),
                window_background: WindowBackgroundAppearance::Opaque,
                app_id: Some("ai.kverona.sluice".into()),
                ..Default::default()
            };
            cx.open_window(options, |window, cx| {
                let workbench = cx.new(|cx| {
                    let mut w = Workbench::new(repo, window, cx);
                    w.attach_ipc(ipc_rx, cx);
                    w
                });
                cx.new(|cx| Root::new(gpui::AnyView::from(workbench), window, cx))
            })
            .expect("failed to open the Sluice window");
            cx.activate(true);
        });
    Ok(())
}
