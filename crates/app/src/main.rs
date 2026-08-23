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
    /// Receive an AI tool hook event on stdin and forward it to the running app (M4, 03 §2).
    Hook { tool: String },
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
        Some(Command::Hook { tool }) => not_yet(&format!("sluice hook {tool}"), "M4 — 03 §2"),
        Some(Command::Askpass { .. }) => not_yet("sluice askpass", "M2 — 05 §3"),
        Some(Command::Editor { .. }) => not_yet("sluice editor", "M3 — 05 §6"),
        Some(Command::SeqEditor { .. }) => not_yet("sluice seq-editor", "M3 — 05 §6"),
        Some(Command::Diagnostics) => not_yet("sluice diagnostics", "M1 — 05 §9.2"),
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
    let repo = Repo::open(&path).with_context(|| format!("opening {}", path.display()))?;
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

            let bounds = Bounds::centered(None, size(px(1400.), px(860.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(title.clone().into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(12.), px(8.))),
                }),
                window_min_size: Some(size(px(1100.), px(680.))),
                window_background: WindowBackgroundAppearance::Opaque,
                app_id: Some("ai.kverona.sluice".into()),
                ..Default::default()
            };
            cx.open_window(options, |window, cx| {
                let workbench = cx.new(|cx| Workbench::new(repo, window, cx));
                cx.new(|cx| Root::new(gpui::AnyView::from(workbench), window, cx))
            })
            .expect("failed to open the Sluice window");
            cx.activate(true);
        });
    Ok(())
}
