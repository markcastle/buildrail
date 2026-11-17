//! Shared helpers for the window-level tests.
//!
//! * [`CpuRenderer`] – an `egui_kittest` renderer that rasterises egui's
//!   tessellated meshes on the CPU. The CI container has neither a Vulkan
//!   driver nor EGL, so kittest's default wgpu renderer cannot get an
//!   adapter; this one needs nothing and is plenty for a 24 px strip.
//! * [`Fixture`] – a temp dir, a [`FakeWorld`] and a tokio runtime, from
//!   which a real [`BuildRailUi`] (real controller, real refresh engine,
//!   real file stores) is built.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use buildrail_app::app::AppController;
use buildrail_app::fakes::FakeWorld;
use buildrail_app::ui::BuildRailUi;
use buildrail_core::domain::{BuildRecord, BuildStatus, ConnectionAlias, ProviderKind, RunId};
use buildrail_core::ports::{CredentialWriter, Secret};
use buildrail_core::refresh::RefreshPolicy;
use buildrail_profile::{AppPaths, DevicePrefs, LocalConnection, Profile, WatchDraft};
use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use eframe::egui::{self, epaint, Color32, TextureId, TexturesDelta};
use egui_kittest::{Harness, TestRenderer};
use tempfile::TempDir;
use tokio::runtime::Runtime;

// ------------------------------------------------------------ renderer

/// Software renderer for kittest. See the module docs.
#[derive(Default)]
pub struct CpuRenderer {
    textures: HashMap<TextureId, ([usize; 2], Vec<Color32>)>,
}

impl TestRenderer for CpuRenderer {
    fn handle_delta(&mut self, delta: &mut TexturesDelta) {
        for (id, image_delta) in delta
            .set
            .drain()
            .flat_map(|(id, deltas)| deltas.into_iter().map(move |d| (id, d)))
        {
            let epaint::ImageData::Color(image) = &image_delta.image;
            match image_delta.pos {
                None => {
                    self.textures.insert(id, (image.size, image.pixels.clone()));
                }
                Some([x0, y0]) => {
                    if let Some((size, pixels)) = self.textures.get_mut(&id) {
                        for y in 0..image.size[1] {
                            for x in 0..image.size[0] {
                                let (tx, ty) = (x0 + x, y0 + y);
                                if tx < size[0] && ty < size[1] {
                                    pixels[ty * size[0] + tx] = image.pixels[y * image.size[0] + x];
                                }
                            }
                        }
                    }
                }
            }
        }
        for id in delta.free.drain() {
            self.textures.remove(&id);
        }
    }

    fn render(
        &mut self,
        ctx: &egui::Context,
        output: &egui::FullOutput,
    ) -> Result<image::RgbaImage, String> {
        let ppp = output.pixels_per_point;
        let screen = ctx.content_rect();
        let width = (screen.width() * ppp).round() as usize;
        let height = (screen.height() * ppp).round() as usize;
        let mut buffer = vec![[0.0f32; 4]; width * height];
        for primitive in ctx.tessellate(output.shapes.clone(), ppp) {
            let epaint::Primitive::Mesh(mesh) = &primitive.primitive else {
                continue;
            };
            let clip = primitive.clip_rect;
            let clip = [
                ((clip.min.x * ppp).floor().max(0.0)) as usize,
                ((clip.min.y * ppp).floor().max(0.0)) as usize,
                ((clip.max.x * ppp).ceil().min(width as f32)) as usize,
                ((clip.max.y * ppp).ceil().min(height as f32)) as usize,
            ];
            let texture = self.textures.get(&mesh.texture_id);
            for tri in mesh.indices.as_chunks::<3>().0 {
                let v = [
                    &mesh.vertices[tri[0] as usize],
                    &mesh.vertices[tri[1] as usize],
                    &mesh.vertices[tri[2] as usize],
                ];
                raster_triangle(&mut buffer, width, clip, ppp, v, texture);
            }
        }
        let mut image = image::RgbaImage::new(width as u32, height as u32);
        for (i, px) in buffer.iter().enumerate() {
            let a = px[3].clamp(0.0, 1.0);
            // Un-premultiply for the PNG.
            let un = |c: f32| {
                if a > 0.0 {
                    (c / a * 255.0).round().clamp(0.0, 255.0) as u8
                } else {
                    0
                }
            };
            image.put_pixel(
                (i % width) as u32,
                (i / width) as u32,
                image::Rgba([un(px[0]), un(px[1]), un(px[2]), (a * 255.0).round() as u8]),
            );
        }
        Ok(image)
    }
}

fn sample(texture: Option<&([usize; 2], Vec<Color32>)>, u: f32, v: f32) -> [f32; 4] {
    let Some((size, pixels)) = texture else {
        return [1.0; 4];
    };
    let (w, h) = (size[0] as f32, size[1] as f32);
    let x = (u * w - 0.5).clamp(0.0, w - 1.0);
    let y = (v * h - 0.5).clamp(0.0, h - 1.0);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(size[0] - 1), (y0 + 1).min(size[1] - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let get = |x: usize, y: usize| {
        let c = pixels[y * size[0] + x];
        [c.r(), c.g(), c.b(), c.a()].map(|v| f32::from(v) / 255.0)
    };
    let (a, b, c, d) = (get(x0, y0), get(x1, y0), get(x0, y1), get(x1, y1));
    let mut out = [0.0; 4];
    for i in 0..4 {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bottom = c[i] + (d[i] - c[i]) * fx;
        out[i] = top + (bottom - top) * fy;
    }
    out
}

fn raster_triangle(
    buffer: &mut [[f32; 4]],
    width: usize,
    clip: [usize; 4],
    ppp: f32,
    v: [&epaint::Vertex; 3],
    texture: Option<&([usize; 2], Vec<Color32>)>,
) {
    let p: [(f32, f32); 3] = v.map(|v| (v.pos.x * ppp, v.pos.y * ppp));
    let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
    if area.abs() < 1e-6 {
        return;
    }
    let min_x = p
        .iter()
        .map(|q| q.0)
        .fold(f32::MAX, f32::min)
        .floor()
        .max(clip[0] as f32) as usize;
    let max_x = p
        .iter()
        .map(|q| q.0)
        .fold(f32::MIN, f32::max)
        .ceil()
        .min(clip[2] as f32) as usize;
    let min_y = p
        .iter()
        .map(|q| q.1)
        .fold(f32::MAX, f32::min)
        .floor()
        .max(clip[1] as f32) as usize;
    let max_y = p
        .iter()
        .map(|q| q.1)
        .fold(f32::MIN, f32::max)
        .ceil()
        .min(clip[3] as f32) as usize;
    let colors = v.map(|v| {
        [v.color.r(), v.color.g(), v.color.b(), v.color.a()].map(|c| f32::from(c) / 255.0)
    });
    for y in min_y..max_y {
        for x in min_x..max_x {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let edge =
                |a: (f32, f32), b: (f32, f32)| (b.0 - a.0) * (py - a.1) - (b.1 - a.1) * (px - a.0);
            let w0 = edge(p[1], p[2]) / area;
            let w1 = edge(p[2], p[0]) / area;
            let w2 = edge(p[0], p[1]) / area;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
            let t = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
            let tex = sample(texture, u, t);
            let dst = &mut buffer[y * width + x];
            let mut src = [0.0; 4];
            for i in 0..4 {
                src[i] = (w0 * colors[0][i] + w1 * colors[1][i] + w2 * colors[2][i]) * tex[i];
            }
            let keep = 1.0 - src[3];
            for i in 0..4 {
                dst[i] = src[i] + dst[i] * keep;
            }
        }
    }
}

// ------------------------------------------------------------- fixture

/// The instant the fake clock starts at.
pub fn t0() -> DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000, 0).unwrap()
}

/// A record created `minutes_ago` before [`t0`].
pub fn record(
    run: &str,
    attempt: u32,
    status: BuildStatus,
    minutes_ago: i64,
    repo: &str,
) -> BuildRecord {
    let at = t0() - TimeDelta::minutes(minutes_ago);
    BuildRecord {
        run_id: RunId::new(run),
        attempt,
        status,
        url: format!("https://github.com/acme/{repo}/actions/runs/{run}"),
        branch: Some("main".into()),
        commit: Some("a7c412e9f0b1c2d3e4f5".into()),
        failing_job: (status == BuildStatus::Failed).then(|| "test".to_owned()),
        created_at: at,
        updated_at: at,
        observed_at: at,
    }
}

/// A GitHub draft on connection `work`.
pub fn gh(label: &str, repo: &str) -> WatchDraft {
    WatchDraft::github(label, "work", "acme", repo, "ci.yml", Some("main".into())).with_id(repo)
}

/// A Cloudflare draft on connection `cf`.
pub fn cf(label: &str, worker: &str) -> WatchDraft {
    WatchDraft::cloudflare(
        label,
        "cf",
        "0123456789abcdef0123456789abcdef",
        worker,
        None,
    )
    .with_id(worker)
}

/// A refresh policy fast enough for wall-clock tests.
pub fn fast_policy() -> RefreshPolicy {
    RefreshPolicy {
        idle: Duration::from_millis(100),
        active: Duration::from_millis(100),
        backoff_base: Duration::from_millis(100),
        backoff_max: Duration::from_millis(100),
        ..RefreshPolicy::default()
    }
}

/// Files, fakes and a runtime for one test.
pub struct Fixture {
    pub dir: TempDir,
    pub paths: AppPaths,
    pub world: FakeWorld,
    /// `None` in async tests, which use the test's own runtime.
    pub runtime: Option<Runtime>,
}

impl Fixture {
    /// An empty machine (no profile, no connections).
    pub fn new() -> Self {
        Self::build(Some(Runtime::new().unwrap()))
    }

    /// Like [`Self::new`] but without a runtime of its own (async tests
    /// use theirs; dropping a runtime inside one would panic).
    pub fn for_async() -> Self {
        Self::build(None)
    }

    fn build(runtime: Option<Runtime>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_dirs(dir.path(), dir.path(), dir.path().join("cache"));
        Self {
            dir,
            paths,
            world: FakeWorld::new(t0()),
            runtime,
        }
    }

    /// The fixture's runtime handle.
    pub fn handle(&self) -> tokio::runtime::Handle {
        self.runtime
            .as_ref()
            .expect("fixture without runtime")
            .handle()
            .clone()
    }

    /// A machine with `drafts` pinned and connections `work` (GitHub) and
    /// `cf` (Cloudflare) set up with stored tokens.
    pub fn with_profile(drafts: &[WatchDraft]) -> Self {
        let fixture = Self::new();
        fixture
            .paths
            .profile_store()
            .save(&Profile::from_drafts(drafts).unwrap())
            .unwrap();
        let mut prefs = DevicePrefs::default();
        for (alias, provider) in [
            ("work", ProviderKind::GitHub),
            ("cf", ProviderKind::Cloudflare),
        ] {
            prefs.connections.insert(
                ConnectionAlias::new(alias),
                LocalConnection {
                    provider,
                    display_name: format!("{alias}-user"),
                },
            );
            fixture
                .world
                .credentials
                .write(&ConnectionAlias::new(alias), &Secret::new("token"))
                .unwrap();
        }
        fixture.paths.device_prefs_store().save(&prefs).unwrap();
        fixture
    }

    /// Starts a controller and wraps it in the UI (animation off).
    pub fn ui(&self) -> BuildRailUi {
        let controller = AppController::start(
            self.world.services(&self.paths),
            Arc::new(self.paths.snapshot_cache()),
            fast_policy(),
            self.handle(),
        );
        let mut ui = BuildRailUi::new(controller);
        ui.set_animate(false);
        ui
    }

    /// A harness of `size` points around [`Self::ui`].
    pub fn harness(&self, size: [f32; 2]) -> Harness<'static, BuildRailUi> {
        Harness::builder()
            .with_size(size)
            .renderer(CpuRenderer::default())
            .build_ui_state(root_ui, self.ui())
    }
}

/// Runs one frame of the app in a margin-less Ui covering the whole
/// window, like eframe's root Ui (kittest adds an 8 pt margin otherwise).
pub fn root_ui(ui: &mut egui::Ui, app: &mut BuildRailUi) {
    let full = ui.ctx().content_rect();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(full));
    child.set_clip_rect(full);
    app.ui(&mut child);
}

/// Steps the harness until `check` holds (the refresh engine and jobs run
/// on real threads), or panics after a few seconds.
pub fn step_until(
    harness: &mut Harness<'_, BuildRailUi>,
    what: &str,
    check: impl Fn(&Harness<'_, BuildRailUi>) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        harness.step();
        if check(harness) {
            harness.step();
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Labels of every node with this role whose label contains `part`,
/// ordered left to right.
pub fn labels_left_to_right(
    harness: &Harness<'_, BuildRailUi>,
    role: egui::accesskit::Role,
) -> Vec<String> {
    use egui_kittest::kittest::{NodeT, Queryable};
    let mut nodes: Vec<(f32, String)> = harness
        .query_all_by_role(role)
        .filter(|n| n.rect().min.y < 24.0)
        .map(|n| {
            (
                n.rect().min.x,
                n.accesskit_node().label().unwrap_or_default(),
            )
        })
        .collect();
    nodes.sort_by(|a, b| a.0.total_cmp(&b.0));
    nodes.into_iter().map(|(_, l)| l).collect()
}

/// Saves a rendered frame as PNG.
pub fn save_png(image: &image::RgbaImage, path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    image.save(path).unwrap();
}

// --------------------------------------------------------------- demo

/// A UI showing the demo profile (four GitHub workflows and Cloudflare
/// Workers cycling through states) at a fixed instant, in `theme`.
pub fn demo_ui(f: &Fixture, theme: buildrail_profile::Theme) -> BuildRailUi {
    use buildrail_app::demo::{demo_connections, demo_profile, DemoRunSource, PHASE_SECS};
    use buildrail_app::fakes::ManualClock;
    use buildrail_core::refresh::SourceRegistry;

    let clock = Arc::new(ManualClock::new(t0()));
    let epoch = t0() - TimeDelta::seconds(PHASE_SECS * 3);
    f.paths
        .profile_store()
        .save(&demo_profile().unwrap())
        .unwrap();
    let mut prefs = DevicePrefs::default();
    for (alias, connection) in demo_connections() {
        prefs
            .connections
            .insert(ConnectionAlias::new(alias), connection);
        f.world
            .credentials
            .write(&ConnectionAlias::new(alias), &Secret::new("t"))
            .unwrap();
    }
    prefs.theme = theme;
    f.paths.device_prefs_store().save(&prefs).unwrap();
    let mut services = f.world.services(&f.paths);
    services.clock = clock.clone();
    services.sources = SourceRegistry::new()
        .with(Arc::new(DemoRunSource::new(
            ProviderKind::GitHub,
            clock.clone(),
            epoch,
        )))
        .with(Arc::new(DemoRunSource::new(
            ProviderKind::Cloudflare,
            clock,
            epoch,
        )));
    let controller = AppController::start(
        services,
        Arc::new(f.paths.snapshot_cache()),
        fast_policy(),
        f.handle(),
    );
    let mut app = BuildRailUi::new(controller);
    app.set_animate(false);
    app
}

/// `true` once every demo entry has its first result.
pub fn demo_loaded(h: &Harness<'_, BuildRailUi>) -> bool {
    use egui_kittest::kittest::Queryable;
    h.query_by_label_contains("Checking").is_none()
        && h.query_by_label_contains("Docs, Cloudflare, No runs yet")
            .is_some()
}

/// Where screenshots go: `docs/images/` when `BUILDRAIL_WRITE_SCREENSHOT=1`,
/// else the test's temp dir.
pub fn screenshot_path(name: &str) -> std::path::PathBuf {
    if std::env::var("BUILDRAIL_WRITE_SCREENSHOT").as_deref() == Ok("1") {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/images")
            .join(name)
    } else {
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
    }
}

/// The text field named `name` (a row's visible label has the same name).
pub fn field<'h>(h: &'h Harness<'_, BuildRailUi>, name: &'h str) -> egui_kittest::Node<'h> {
    use egui::accesskit::Role;
    use egui_kittest::kittest::{NodeT, Queryable};
    h.query_all_by_label(name)
        .find(|n| {
            matches!(
                n.accesskit_node().role(),
                Role::TextInput | Role::PasswordInput | Role::MultilineTextInput
            )
        })
        .unwrap_or_else(|| panic!("no field {name:?}"))
}

/// Focuses the field `name` and types `text` into it.
pub fn type_into(h: &mut Harness<'_, BuildRailUi>, name: &str, text: &str) {
    field(h, name).focus();
    for _ in 0..3 {
        h.step();
    }
    field(h, name).type_text(text);
    for _ in 0..3 {
        h.step();
    }
}
