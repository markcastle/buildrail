//! The composition root: the one place that picks concrete adapters.
//!
//! [`run`] parses the command line, initialises logging, builds either the
//! production ports (HTTPS providers, OS credential store, system browser,
//! platform dock) or the `--demo` fakes, starts the controller on a tokio
//! runtime and hands everything to eframe. Nothing else in the crate names
//! a concrete adapter type.

use std::ffi::OsString;
use std::path::PathBuf;
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
use buildrail_platform::fullscreen::platform_fullscreen_probe;
use buildrail_platform::geometry::{MonitorId, MonitorInfo, PhysicalRect};
use buildrail_platform::monitors::{platform_monitor_source, MonitorSource, StaticMonitors};
use buildrail_platform::url_opener::SystemUrlOpener;
use buildrail_profile::{AppPaths, DevicePrefs};
use buildrail_providers::{default_http_client, ProviderSet};
use eframe::egui;
use raw_window_handle::RawWindowHandle;
use thiserror::Error;

use crate::app::{AppController, Catalogs, Services};
use crate::demo::{demo_connections, demo_profile, DemoRunSource, LoggingUrlOpener};
use crate::fakes::FakeCatalog;
use crate::shell::DockController;
use crate::ui::window::{strip_viewport, ShellFactory, StripWindow};
use crate::ui::BuildRailUi;

/// Environment variable naming a directory to keep everything in
/// (profile, device preferences, cache) instead of the OS locations. For
/// tests and portable installs.
pub const CONFIG_DIR_ENV: &str = "BUILDRAIL_CONFIG_DIR";

/// Environment variable choosing the renderer: `glow` (OpenGL) or `wgpu`.
pub const RENDERER_ENV: &str = "BUILDRAIL_RENDERER";

/// Parsed command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Run against the in-memory demo provider.
    pub demo: bool,
    /// Use the wgpu renderer instead of glow.
    pub wgpu: bool,
    /// Print usage and exit.
    pub help: bool,
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
    /// The window could not be created (no display, no GPU driver, …).
    #[error("could not open the window: {0}")]
    Window(String),
}

/// Usage text.
pub const USAGE: &str = "\
BuildRail – a thin desktop strip showing your pinned CI builds.

Usage: buildrail [--demo] [--renderer glow|wgpu]

  --demo            Run with a built-in fake provider and sample builds
                    (no network, no credential store).
  --renderer NAME   Use the glow (OpenGL, default) or wgpu renderer.
  --help            Show this text.

Environment:
  BUILDRAIL_CONFIG_DIR  Keep profile, preferences and cache in this directory.
  BUILDRAIL_RENDERER    Same as --renderer.
  RUST_LOG              Log filter, e.g. RUST_LOG=buildrail_app=debug.";

/// Parses arguments (without the program name).
pub fn parse_args(
    args: impl IntoIterator<Item = OsString>,
    renderer_env: Option<OsString>,
) -> Result<Options, StartError> {
    let mut options = Options {
        wgpu: renderer_env.is_some_and(|r| r == "wgpu"),
        ..Options::default()
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--demo") => options.demo = true,
            Some("--help" | "-h") => options.help = true,
            Some("--renderer") => match args.next().as_ref().and_then(|a| a.to_str()) {
                Some("wgpu") => options.wgpu = true,
                Some("glow") => options.wgpu = false,
                other => {
                    return Err(StartError::Usage(format!(
                        "--renderer {}",
                        other.unwrap_or("")
                    )))
                }
            },
            _ => return Err(StartError::Usage(arg.to_string_lossy().into_owned())),
        }
    }
    Ok(options)
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

/// Starts logging. `RUST_LOG` overrides the default (`info` for BuildRail
/// crates, `warn` otherwise). Secrets never reach the log: `Secret`'s
/// formatting prints `***`, and adapters never log tokens.
pub fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(
            "warn,buildrail_app=info,buildrail_core=info,buildrail_providers=info,buildrail_platform=info,buildrail_profile=info",
        )
    });
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

/// The production ports.
pub fn production_services(paths: &AppPaths) -> Result<Services, StartError> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let client = default_http_client().map_err(|e| StartError::Init(e.to_string()))?;
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
    })
}

/// The demo ports, with the canned profile and connections written to
/// `paths` when no profile exists there yet.
pub fn demo_services(paths: &AppPaths) -> Result<Services, StartError> {
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
/// probe; monitors from the OS where available, else from what egui knows
/// about the window's monitor (Wayland, macOS).
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
            None => {
                let size = monitor_points.unwrap_or(egui::vec2(1280.0, 800.0)) * native_ppp;
                let bounds = PhysicalRect::new(0, 0, size.x.round() as u32, size.y.round() as u32);
                Box::new(StaticMonitors(vec![MonitorInfo::without_work_area(
                    MonitorId::new("current"),
                    bounds,
                    f64::from(native_ppp),
                    true,
                )]))
            }
        };
        DockController::new(
            platform_dock(),
            platform_fullscreen_probe(Some(handle)),
            monitors,
        )
    })
}

/// Runs the app until the window closes.
pub fn run(options: &Options) -> Result<(), StartError> {
    init_logging();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .thread_name("buildrail-worker")
        .build()
        .map_err(|e| StartError::Init(e.to_string()))?;

    // The demo keeps its files in a throw-away directory unless told otherwise.
    let override_dir = std::env::var_os(CONFIG_DIR_ENV);
    let demo_dir = match (options.demo, &override_dir) {
        (true, None) => Some(tempfile::tempdir().map_err(|e| StartError::Init(e.to_string()))?),
        _ => None,
    };
    let paths = match &demo_dir {
        Some(dir) => app_paths(Some(dir.path().as_os_str().to_owned()))?,
        None => app_paths(override_dir)?,
    };
    tracing::info!(profile = %paths.profile_file().display(), demo = options.demo, "starting BuildRail");

    let (services, policy) = if options.demo {
        (demo_services(&paths)?, demo_policy())
    } else {
        (production_services(&paths)?, RefreshPolicy::default())
    };
    let cache: Arc<dyn SnapshotCache> = Arc::new(paths.snapshot_cache());
    let controller = AppController::start(services, cache, policy, runtime.handle().clone());
    let viewport = strip_viewport(&controller.prefs().placement);
    let native = eframe::NativeOptions {
        viewport,
        renderer: if options.wgpu {
            eframe::Renderer::Wgpu
        } else {
            eframe::Renderer::Glow
        },
        persist_window: false,
        ..eframe::NativeOptions::default()
    };
    let result = eframe::run_native(
        "BuildRail",
        native,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            controller.set_repaint(Arc::new(move || ctx.request_repaint()));
            Ok(Box::new(StripWindow::new(
                BuildRailUi::new(controller),
                platform_shell(),
            )))
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
        assert_eq!(parse_args(args(&[]), None).unwrap(), Options::default());
        let o = parse_args(args(&["--demo", "--renderer", "wgpu"]), None).unwrap();
        assert!(o.demo && o.wgpu);
        let o = parse_args(args(&["--renderer", "glow"]), Some("wgpu".into())).unwrap();
        assert!(!o.wgpu);
        assert!(parse_args(args(&[]), Some("wgpu".into())).unwrap().wgpu);
        assert!(parse_args(args(&["-h"]), None).unwrap().help);
        assert!(matches!(
            parse_args(args(&["--nope"]), None),
            Err(StartError::Usage(_))
        ));
        assert!(matches!(
            parse_args(args(&["--renderer"]), None),
            Err(StartError::Usage(_))
        ));
        assert!(USAGE.contains("--demo"));
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
        let services = demo_services(&paths).unwrap();
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
        let again = demo_services(&paths).unwrap();
        assert!(again.profile_store.load().unwrap().is_empty());
        assert_eq!(demo_policy().active, Duration::from_secs(2));
        init_logging();
        let _ = production_services(&paths).unwrap();
    }
}
