//! The composition root: the one place that picks concrete adapters.
//!
//! [`run`] initialises logging (the terminal and a rotating log file),
//! makes sure only one BuildRail runs, builds either the production ports
//! (HTTPS providers, OS credential store, system browser, platform dock,
//! native file dialogs) or the `--demo` fakes, starts the controller on a
//! small tokio runtime and hands everything to eframe (OpenGL through
//! glow). Nothing else in the crate names a concrete adapter type.
//!
//! Updates: [`run`] first removes what an earlier self-update left beside
//! the executable ([`cleanup_after_update`]); the production ports carry
//! the real [`Updater`] (GitHub release feed, verified with the key baked
//! in at build time), `--demo` a fake that reports the build does not
//! update itself, so the demo never touches the network.
//!
//! Everything the app holds (the screen edge, the single-instance lock) is
//! registered with the controller's [`Lifecycle`] and given back on every
//! way out: Quit and closing the window (`on_exit`), Ctrl+C and SIGTERM
//! (a signal task closes the window), a panic unwinding through [`run`]
//! ([`ReleaseOnDrop`]), and before a self-update restarts the app
//! ([`AppController::prepare_for_restart`]).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use buildrail_core::domain::ConnectionAlias;
use buildrail_core::ports::{Clock, Secret, SnapshotCache, SystemClock};
use buildrail_core::refresh::{RefreshPolicy, SourceRegistry};
use buildrail_platform::autostart::{
    platform_autostart, Autostart, InMemoryAutostart, LaunchCommand,
};
use buildrail_platform::credentials::{InMemoryCredentialStore, KeyringCredentialStore};
use buildrail_platform::dock::platform_dock;
use buildrail_platform::folders::SystemFolderOpener;
use buildrail_platform::fullscreen::platform_fullscreen_probe;
use buildrail_platform::geometry::{MonitorId, MonitorInfo, PhysicalRect};
use buildrail_platform::monitors::{
    main_thread_monitors, platform_monitor_source, MonitorSource, StaticMonitors,
};
use buildrail_platform::single_instance::{
    platform_single_instance, InstanceError, SingleInstance,
};
use buildrail_platform::url_opener::SystemUrlOpener;
use buildrail_profile::{AppPaths, DevicePrefs};
use buildrail_providers::{default_http_client, ProviderSet};
use buildrail_update::{cleanup_after_update, Updater};
use eframe::egui;
use raw_window_handle::RawWindowHandle;
use thiserror::Error;
use tokio::runtime::Handle;

use crate::app::{
    AppController, Catalogs, InstanceLock, Lifecycle, NoUpdates, ReleaseOnDrop, Services,
    UpdateService,
};
use crate::demo::{demo_connections, demo_profile, DemoRunSource, LoggingUrlOpener};
use crate::dialogs::RfdFileDialog;
use crate::fakes::{FakeCatalog, ScriptedUpdateService};
use crate::shell::DockController;
use crate::ui::window::{strip_viewport, ShellFactory, StripWindow};
use crate::ui::{BuildRailUi, FrameCounter};

/// Environment variable naming a directory to keep everything in
/// (profile, device preferences, cache, logs) instead of the OS
/// locations. For tests and portable installs.
pub const CONFIG_DIR_ENV: &str = "BUILDRAIL_CONFIG_DIR";

/// At most this many daily log files are kept.
pub const MAX_LOG_FILES: usize = 5;

/// Parsed command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Run against the in-memory demo provider.
    pub demo: bool,
    /// Print usage and exit.
    pub help: bool,
    /// Print the version and exit.
    pub version: bool,
    /// One-line notes for the terminal (an option that no longer does
    /// anything, for example).
    pub notes: Vec<String>,
}

/// Bad command line or failed start-up.
#[derive(Debug, Error)]
pub enum StartError {
    /// Unknown argument.
    #[error("unknown argument {0:?}; try --help")]
    Usage(String),
    /// No configuration directory could be found.
    #[error(transparent)]
    Paths(#[from] buildrail_profile::NoHomeDir),
    /// The async runtime or HTTP client could not start.
    #[error("could not start: {0}")]
    Init(String),
    /// The window could not be created (no display, no OpenGL driver, …).
    #[error("could not open the window: {0}")]
    Window(String),
}

/// Usage text.
pub const USAGE: &str = "\
BuildRail – a thin desktop strip showing your pinned CI builds.

Usage: buildrail [--demo]

  --demo            Run with a built-in fake provider and sample builds
                    (no network, no credential store).
  --version         Show the version.
  --help            Show this text.

Environment:
  BUILDRAIL_CONFIG_DIR  Keep profile, preferences, cache and logs in this
                        directory.
  RUST_LOG              Log filter, e.g. RUST_LOG=buildrail_app=debug.";

/// `buildrail 0.2.0`.
pub fn version_text() -> String {
    format!("buildrail {}", env!("CARGO_PKG_VERSION"))
}

/// Parses arguments (without the program name).
///
/// `--renderer` (from 0.2.0 shortcuts) is still accepted with or without
/// a value, and only adds a note: BuildRail always draws with OpenGL now.
pub fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Options, StartError> {
    let mut options = Options::default();
    let mut args = args.into_iter().peekable();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--demo") => options.demo = true,
            Some("--help" | "-h") => options.help = true,
            Some("--version" | "-V") => options.version = true,
            Some("--renderer") => {
                let value = args
                    .next_if(|next| !next.to_string_lossy().starts_with('-'))
                    .map(|v| v.to_string_lossy().into_owned());
                options.notes.push(renderer_note(value.as_deref()));
            }
            Some(other) if other.starts_with("--renderer=") => {
                options
                    .notes
                    .push(renderer_note(other.split_once('=').map(|(_, v)| v)));
            }
            _ => return Err(StartError::Usage(arg.to_string_lossy().into_owned())),
        }
    }
    Ok(options)
}

fn renderer_note(value: Option<&str>) -> String {
    let named = value.map_or(String::new(), |v| format!(" {v}"));
    format!("note: --renderer{named} is no longer used; BuildRail always draws with OpenGL.")
}

/// The single-instance lock name: one for the real strip, another for
/// `--demo` (so the two can run side by side), and one per directory when
/// [`CONFIG_DIR_ENV`] points somewhere else (a portable install).
pub fn instance_name(demo: bool, config_dir: Option<&Path>) -> String {
    let base = if demo { "BuildRail-demo" } else { "BuildRail" };
    match config_dir {
        Some(dir) => format!("{base}-{:016x}", fnv1a(dir.to_string_lossy().as_bytes())),
        None => base.to_owned(),
    }
}

/// A stable (across runs and builds) 64-bit FNV-1a hash.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Takes the one-instance lock. `Ok(None)` means another BuildRail holds
/// it; a lock that cannot be checked is logged and ignored (better a
/// second strip than none).
pub fn take_instance_lock(
    locks: &dyn SingleInstance,
    name: &str,
) -> Option<Option<Arc<InstanceLock>>> {
    match locks.acquire(name) {
        Ok(guard) => Some(Some(Arc::new(InstanceLock::new(guard)))),
        Err(InstanceError::AlreadyRunning) => None,
        Err(error) => {
            tracing::warn!(%error, "starting without the single-instance check");
            Some(None)
        }
    }
}

/// The file locations: `dir` (from [`CONFIG_DIR_ENV`]) when set, else the
/// OS defaults.
pub fn app_paths(dir: Option<OsString>) -> Result<AppPaths, buildrail_profile::NoHomeDir> {
    match dir.filter(|d| !d.is_empty()) {
        Some(dir) => {
            let dir = PathBuf::from(dir);
            Ok(AppPaths::from_dirs(&dir, &dir, dir.join("cache")))
        }
        None => AppPaths::discover(),
    }
}

/// The default log filter: `info` for BuildRail crates, `warn` otherwise.
pub const DEFAULT_LOG_FILTER: &str = "warn,buildrail_app=info,buildrail_core=info,buildrail_providers=info,buildrail_platform=info,buildrail_profile=info,buildrail_update=info";

/// Starts logging to the terminal and, when `logs_dir` is given, to a
/// daily log file there (`buildrail.YYYY-MM-DD.log`, at most
/// [`MAX_LOG_FILES`] kept). `RUST_LOG` overrides the default filter.
/// Secrets never reach the log: `Secret`'s formatting prints `***`, and
/// adapters never log tokens or authorisation headers.
pub fn init_logging(logs_dir: Option<&Path>) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::Layer;

    let filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER))
    };
    let file = logs_dir.and_then(|dir| match log_file(dir) {
        Ok(appender) => Some(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(appender)
                .with_filter(filter()),
        ),
        Err(error) => {
            eprintln!("buildrail: not writing a log file: {error}");
            None
        }
    });
    let terminal = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(filter());
    let _ = tracing_subscriber::registry()
        .with(terminal)
        .with(file)
        .try_init();
}

/// Sends panics to the log (and so to the log file) before the default
/// report, so a crash on a machine without a terminal leaves a trace.
pub fn log_panics() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(%info, "BuildRail crashed");
        default(info);
    }));
}

/// The rotating log file in `dir`.
pub fn log_file(
    dir: &Path,
) -> Result<tracing_appender::rolling::RollingFileAppender, tracing_appender::rolling::InitError> {
    // The appender prunes old files by listing the folder first, and
    // complains on stderr when there is no folder yet.
    if let Err(error) = std::fs::create_dir_all(dir) {
        tracing::debug!(%error, "cannot create the log folder");
    }
    tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("buildrail")
        .filename_suffix("log")
        .max_log_files(MAX_LOG_FILES)
        .build(dir)
}

/// Logs, at debug level once a minute, how many frames were painted. An
/// idle strip should say 0.
pub async fn report_frames(frames: FrameCounter) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.tick().await;
    let mut last = frames.total();
    loop {
        interval.tick().await;
        let total = frames.total();
        tracing::debug!("painted {} frames in the last minute", total - last);
        last = total;
    }
}

/// Resolves on Ctrl+C, and on SIGTERM where there is one. Never resolves
/// when the signals cannot be watched.
pub async fn termination_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let Ok(mut term) = signal(SignalKind::terminate()) else {
            return ctrl_c().await;
        };
        tokio::select! {
            () = ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        ctrl_c().await;
    }
}

async fn ctrl_c() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// The running version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Removes what an earlier self-update left beside the executable (the
/// moved-aside old copy on Windows, a half-written file after a crash)
/// and logs what happened. Call early, once.
pub fn clean_up_after_update() {
    match cleanup_after_update() {
        Ok(true) => tracing::info!("removed the files a previous update left behind"),
        Ok(false) => tracing::debug!("nothing left behind by an update"),
        Err(error) => {
            tracing::warn!(%error, "could not remove the files a previous update left behind")
        }
    }
}

/// The real update service, or one that never updates when the updater
/// could not be set up (logged).
pub fn production_updates(
    updater: Result<Updater, buildrail_update::SetupError>,
) -> Arc<dyn UpdateService> {
    match updater {
        Ok(updater) => Arc::new(updater),
        Err(error) => {
            tracing::warn!(%error, "self-update is unavailable");
            Arc::new(NoUpdates::new(
                buildrail_update::Version::parse(VERSION)
                    .unwrap_or_else(|_| buildrail_update::Version::new(0, 0, 0)),
            ))
        }
    }
}

/// The production ports. Dialog work runs on `runtime`.
pub fn production_services(paths: &AppPaths, runtime: &Handle) -> Result<Services, StartError> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let client = default_http_client().map_err(|e| StartError::Init(e.to_string()))?;
    // One HTTP client (and connection pool) for providers and updates.
    let updates = production_updates(Updater::for_this_app(client.clone(), VERSION));
    let providers = ProviderSet::new(client, clock.clone());
    let autostart: Option<Arc<dyn Autostart>> = LaunchCommand::current()
        .and_then(platform_autostart)
        .map(Arc::from)
        .map_err(|error| tracing::warn!(%error, "launch at login is unavailable"))
        .ok();
    Ok(Services {
        sources: providers.run_sources().into_iter().collect(),
        catalogs: Catalogs::from_pair(
            providers.github_catalog.clone(),
            providers.cloudflare_catalog.clone(),
        ),
        credentials: Arc::new(KeyringCredentialStore::new()),
        clock,
        urls: Arc::new(SystemUrlOpener),
        autostart,
        profile_store: paths.profile_store(),
        prefs_store: paths.device_prefs_store(),
        dialogs: Arc::new(RfdFileDialog::new(runtime.clone())),
        folders: Arc::new(SystemFolderOpener),
        logs_dir: Some(paths.logs_dir().to_owned()),
        updates,
    })
}

/// The demo ports, with the canned profile and connections written to
/// `paths` when no profile exists there yet. Dialogs are real (they only
/// touch files the person picks).
pub fn demo_services(paths: &AppPaths, runtime: &Handle) -> Result<Services, StartError> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let epoch = clock.now();
    let sources: SourceRegistry = [
        Arc::new(DemoRunSource::new(
            buildrail_core::domain::ProviderKind::GitHub,
            clock.clone(),
            epoch,
        )) as Arc<dyn buildrail_core::ports::RunSource>,
        Arc::new(DemoRunSource::new(
            buildrail_core::domain::ProviderKind::Cloudflare,
            clock.clone(),
            epoch,
        )),
    ]
    .into_iter()
    .collect();
    let credentials = InMemoryCredentialStore::with_entries(
        demo_connections()
            .into_iter()
            .map(|(alias, _)| (ConnectionAlias::new(alias), Secret::new("demo-token"))),
    );
    let profile_store = paths.profile_store();
    let prefs_store = paths.device_prefs_store();
    if !profile_store.path().exists() {
        let init = || -> Result<(), String> {
            let profile = demo_profile().map_err(|e| e.to_string())?;
            profile_store.save(&profile).map_err(|e| e.to_string())?;
            let mut prefs = prefs_store
                .load()
                .unwrap_or_else(|_| DevicePrefs::default());
            for (alias, connection) in demo_connections() {
                prefs
                    .connections
                    .insert(ConnectionAlias::new(alias), connection);
            }
            prefs_store.save(&prefs).map_err(|e| e.to_string())
        };
        init().map_err(StartError::Init)?;
    }
    let catalog = Arc::new(FakeCatalog::default());
    Ok(Services {
        sources,
        catalogs: Catalogs::from_pair(catalog.clone(), catalog),
        credentials: Arc::new(credentials),
        clock,
        urls: Arc::new(LoggingUrlOpener),
        autostart: Some(Arc::new(InMemoryAutostart::new(false))),
        profile_store,
        prefs_store,
        dialogs: Arc::new(RfdFileDialog::new(runtime.clone())),
        folders: Arc::new(SystemFolderOpener),
        logs_dir: Some(paths.logs_dir().to_owned()),
        // The demo never touches the network: it does not update itself.
        updates: Arc::new(ScriptedUpdateService::not_configured(VERSION)),
    })
}

/// Refresh cadence for the demo: fast, so states visibly change.
pub fn demo_policy() -> RefreshPolicy {
    RefreshPolicy {
        idle: Duration::from_secs(3),
        active: Duration::from_secs(2),
        backoff_base: Duration::from_secs(3),
        backoff_max: Duration::from_secs(10),
        ..RefreshPolicy::default()
    }
}

/// The dock controller for the real window: platform dock and fullscreen
/// probe; monitors from the OS where available (on macOS read once, here,
/// on the main thread), else from what egui knows about the window's
/// monitor (Wayland).
pub fn platform_shell() -> ShellFactory {
    Box::new(|handle: &RawWindowHandle, ctx: &egui::Context| {
        let (native_ppp, monitor_points) = ctx.input(|i| {
            (
                i.viewport().native_pixels_per_point.unwrap_or(1.0),
                i.viewport().monitor_size,
            )
        });
        let monitors: Box<dyn MonitorSource> = match platform_monitor_source(f64::from(native_ppp))
        {
            Some(source) => source,
            // macOS: the screens with their visible frames (above the Dock,
            // below the menu bar), read here because this is the main thread.
            None => match main_thread_monitors().filter(|list| !list.is_empty()) {
                Some(list) => Box::new(StaticMonitors(list)),
                None => {
                    let size = monitor_points.unwrap_or(egui::vec2(1280.0, 800.0)) * native_ppp;
                    let bounds =
                        PhysicalRect::new(0, 0, size.x.round() as u32, size.y.round() as u32);
                    Box::new(StaticMonitors(vec![MonitorInfo::without_work_area(
                        MonitorId::new("current"),
                        bounds,
                        f64::from(native_ppp),
                        true,
                    )]))
                }
            },
        };
        DockController::new(
            platform_dock(),
            platform_fullscreen_probe(Some(handle)),
            monitors,
        )
    })
}

/// The tokio runtime for provider calls and jobs: two workers and a few
/// blocking threads (credential store calls) are plenty for a handful of
/// HTTPS requests a minute.
pub fn runtime() -> Result<tokio::runtime::Runtime, StartError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .max_blocking_threads(4)
        .thread_name("buildrail-worker")
        .build()
        .map_err(|e| StartError::Init(e.to_string()))
}

/// Where the single-instance lock file lives (not used on Windows, which
/// uses a named mutex): the cache directory of the real locations, so the
/// `--demo` strip (whose files are in a throw-away directory) is still
/// found by the next `--demo`.
fn lock_dir(override_dir: Option<&Path>) -> Result<PathBuf, StartError> {
    Ok(match override_dir {
        Some(dir) => dir.to_owned(),
        None => AppPaths::discover()?.cache_dir().to_owned(),
    })
}

/// Runs the app until the window closes. Returns at once (successfully)
/// when another BuildRail is already running.
pub fn run(options: &Options) -> Result<(), StartError> {
    // The demo keeps its files in a throw-away directory unless told otherwise.
    let override_dir = std::env::var_os(CONFIG_DIR_ENV).filter(|d| !d.is_empty());
    let demo_dir = match (options.demo, &override_dir) {
        (true, None) => Some(tempfile::tempdir().map_err(|e| StartError::Init(e.to_string()))?),
        _ => None,
    };
    let paths = match &demo_dir {
        Some(dir) => app_paths(Some(dir.path().as_os_str().to_owned()))?,
        None => app_paths(override_dir.clone())?,
    };
    init_logging(Some(paths.logs_dir()));
    log_panics();
    clean_up_after_update();

    let override_path = override_dir.as_deref().map(Path::new);
    let name = instance_name(options.demo, override_path);
    let locks = platform_single_instance(&lock_dir(override_path)?);
    let Some(instance) = take_instance_lock(locks.as_ref(), &name) else {
        tracing::info!("BuildRail is already running; not starting another strip");
        return Ok(());
    };

    let runtime = runtime()?;
    tracing::info!(
        version = VERSION,
        profile = %paths.profile_file().display(),
        demo = options.demo,
        "starting BuildRail"
    );
    let (services, policy) = if options.demo {
        (demo_services(&paths, runtime.handle())?, demo_policy())
    } else {
        (
            production_services(&paths, runtime.handle())?,
            RefreshPolicy::default(),
        )
    };
    let cache: Arc<dyn SnapshotCache> = Arc::new(paths.snapshot_cache());
    let controller = AppController::start(services, cache, policy, runtime.handle().clone());
    let lifecycle: Lifecycle = controller.lifecycle().clone();
    if let Some(instance) = instance {
        lifecycle.hold(instance);
    }
    // Whatever happens from here (return, error, panic unwinding), give
    // back the screen edge and the lock.
    let _release = ReleaseOnDrop(lifecycle.clone());

    let viewport = strip_viewport(&controller.prefs().placement);
    let native = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        persist_window: false,
        ..eframe::NativeOptions::default()
    };
    let handle = runtime.handle().clone();
    let result = eframe::run_native(
        "BuildRail",
        native,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            let wake = ctx.clone();
            controller.set_repaint(Arc::new(move || wake.request_repaint()));
            let close = ctx.clone();
            let on_signal = lifecycle.clone();
            handle.spawn(async move {
                termination_signal().await;
                tracing::info!("asked to stop; closing");
                let _ = tokio::task::spawn_blocking(move || on_signal.release_all()).await;
                close.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
                close.request_repaint();
                // A window that does not close (a hung driver) must not keep
                // the process alive; everything held is already released.
                tokio::time::sleep(Duration::from_secs(5)).await;
                tracing::warn!("the window did not close; exiting");
                std::process::exit(1);
            });
            let mut ui = BuildRailUi::new(controller);
            ui.set_reduced_motion(
                buildrail_platform::motion::prefers_reduced_motion().unwrap_or(false),
            );
            handle.spawn(report_frames(ui.frame_counter()));
            Ok(Box::new(StripWindow::new(ui, platform_shell())))
        }),
    );
    drop(demo_dir);
    result.map_err(|e| StartError::Window(e.to_string()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_flags() {
        assert_eq!(parse_args(args(&[])).unwrap(), Options::default());
        let o = parse_args(args(&["--demo", "--version"])).unwrap();
        assert!(o.demo && o.version && o.notes.is_empty());
        assert!(parse_args(args(&["-h"])).unwrap().help);
        assert!(parse_args(args(&["-V"])).unwrap().version);
        assert!(matches!(
            parse_args(args(&["--nope"])),
            Err(StartError::Usage(_))
        ));
        assert!(USAGE.contains("--demo") && USAGE.contains("--version"));
        assert!(!USAGE.contains("--renderer"));
        assert!(version_text().starts_with("buildrail 0."));
    }

    #[test]
    fn legacy_renderer_flag_is_a_note() {
        for list in [
            &["--renderer", "wgpu", "--demo"][..],
            &["--renderer=glow", "--demo"],
            &["--renderer", "--demo"],
        ] {
            let o = parse_args(args(list)).unwrap();
            assert!(o.demo, "{list:?}");
            assert_eq!(o.notes.len(), 1, "{list:?}");
            assert!(o.notes[0].contains("no longer used"), "{}", o.notes[0]);
        }
        let o = parse_args(args(&["--renderer"])).unwrap();
        assert_eq!(
            o.notes,
            ["note: --renderer is no longer used; BuildRail always draws with OpenGL."]
        );
    }

    #[test]
    fn instance_names_separate_demo_and_portable_installs() {
        assert_eq!(instance_name(false, None), "BuildRail");
        assert_eq!(instance_name(true, None), "BuildRail-demo");
        let a = instance_name(false, Some(Path::new("/a")));
        assert_eq!(a, instance_name(false, Some(Path::new("/a"))));
        assert_ne!(a, instance_name(false, Some(Path::new("/b"))));
        assert!(a.starts_with("BuildRail-"));
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn second_instance_is_turned_away() {
        use buildrail_platform::single_instance::InMemoryInstances;

        let locks = InMemoryInstances::new();
        let first = take_instance_lock(&locks, "BuildRail").unwrap().unwrap();
        assert!(take_instance_lock(&locks, "BuildRail").is_none());
        // The demo is a different strip.
        assert!(take_instance_lock(&locks, "BuildRail-demo").is_some());
        // Releasing (exit, or before a restart) lets the next one in.
        crate::app::Held::release(first.as_ref()).unwrap();
        assert!(take_instance_lock(&locks, "BuildRail").is_some());

        struct Broken;
        impl SingleInstance for Broken {
            fn acquire(
                &self,
                _: &str,
            ) -> Result<buildrail_platform::single_instance::InstanceGuard, InstanceError>
            {
                Err(InstanceError::Io("read-only".into()))
            }
        }
        assert!(matches!(take_instance_lock(&Broken, "x"), Some(None)));
    }

    #[test]
    fn log_files_go_to_the_logs_dir() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        let appender = log_file(&logs).unwrap();
        drop(appender);
        assert!(logs.is_dir());
        // A file where the folder should be cannot hold logs.
        let blocked = dir.path().join("file");
        std::fs::write(&blocked, b"x").unwrap();
        assert!(log_file(&blocked.join("logs")).is_err());
        assert!(lock_dir(Some(dir.path())).unwrap() == dir.path());
        let rt = runtime().unwrap();
        rt.block_on(async {
            let frames = FrameCounter::new();
            frames.record();
            // The reporter runs until dropped.
            let reporter = tokio::spawn(report_frames(frames));
            tokio::task::yield_now().await;
            reporter.abort();
            assert!(
                tokio::time::timeout(Duration::from_millis(10), termination_signal())
                    .await
                    .is_err(),
                "no signal arrived"
            );
        });
    }

    #[test]
    fn config_dir_override() {
        let paths = app_paths(Some("/tmp/br".into())).unwrap();
        assert_eq!(paths.profile_file(), PathBuf::from("/tmp/br/profile.toml"));
        assert_eq!(
            paths.device_prefs_file(),
            PathBuf::from("/tmp/br/device.toml")
        );
        assert_eq!(
            paths.snapshot_cache_file(),
            PathBuf::from("/tmp/br/cache/snapshots.json")
        );
        // Empty means "not set".
        assert_eq!(app_paths(Some("".into())).ok(), AppPaths::discover().ok());
    }

    #[test]
    fn demo_services_seed_a_profile_once() {
        let dir = tempfile::tempdir().unwrap();
        let paths = app_paths(Some(dir.path().as_os_str().to_owned())).unwrap();
        let rt = runtime().unwrap();
        let services = demo_services(&paths, rt.handle()).unwrap();
        let profile = services.profile_store.load().unwrap();
        assert_eq!(profile.len(), 8);
        let prefs = services.prefs_store.load().unwrap();
        assert_eq!(prefs.connections.len(), 2);
        assert!(services
            .credentials
            .read(&ConnectionAlias::new("work"))
            .unwrap()
            .is_some());
        // A second start keeps the user's edits.
        services
            .profile_store
            .save(&buildrail_profile::Profile::empty())
            .unwrap();
        let again = demo_services(&paths, rt.handle()).unwrap();
        assert!(again.profile_store.load().unwrap().is_empty());
        assert_eq!(demo_policy().active, Duration::from_secs(2));
        init_logging(Some(paths.logs_dir()));
        let services = production_services(&paths, rt.handle()).unwrap();
        assert_eq!(services.logs_dir.as_deref(), Some(paths.logs_dir()));
    }
}
