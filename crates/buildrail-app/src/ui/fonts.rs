//! The operating system's own interface font, loaded at startup.
//!
//! Task Rail uses "one family, the system's own": Segoe UI on Windows, San
//! Francisco on a Mac. egui bundles its own face, which is much of why a
//! plain egui window looks foreign, so BuildRail looks for the system face
//! on disk and registers its regular, medium, semibold and bold weights as
//! real faces (egui does not synthesise bold).
//!
//! The lookup goes through a small [`FontSource`] seam so the path
//! resolution is tested with a fake file system, and the screenshot tests
//! can point it at a font that exists in a container. Nothing here panics:
//! a missing, unreadable or unparsable file is skipped, and with nothing
//! found the bundled font is used silently.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::epaint::text::VariationCoords;
use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontTweak};

/// egui family name of the medium (500) weight.
pub const MEDIUM: &str = "buildrail-medium";
/// egui family name of the semibold (600) weight.
pub const SEMIBOLD: &str = "buildrail-semibold";
/// egui family name of the bold (700) weight.
pub const BOLD: &str = "buildrail-bold";

/// Reads font files. The real one reads the disk; tests use a map.
pub trait FontSource {
    /// The bytes of `path`, or `None` when it is missing or unreadable.
    fn read(&self, path: &Path) -> Option<Vec<u8>>;
}

/// [`FontSource`] over the real file system.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemFontSource;

impl FontSource for SystemFontSource {
    fn read(&self, path: &Path) -> Option<Vec<u8>> {
        std::fs::read(path).ok()
    }
}

/// A [`FontSource`] over an in-memory map of files, for tests.
#[derive(Debug, Clone, Default)]
pub struct MemoryFontSource {
    files: HashMap<PathBuf, Vec<u8>>,
}

impl MemoryFontSource {
    /// An empty file system.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a file.
    pub fn with(mut self, path: impl Into<PathBuf>, bytes: Vec<u8>) -> Self {
        self.files.insert(path.into(), bytes);
        self
    }
}

impl FontSource for MemoryFontSource {
    fn read(&self, path: &Path) -> Option<Vec<u8>> {
        self.files.get(path).cloned()
    }
}

/// One face in a font file.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceFile {
    /// Where the file is.
    pub path: PathBuf,
    /// The face's index in a collection (`.ttc`), else 0.
    pub index: u32,
    /// For a variable font, the `wght` axis value of this weight.
    pub weight_axis: Option<f32>,
}

impl FaceFile {
    fn at(path: PathBuf) -> Self {
        Self {
            path,
            index: 0,
            weight_axis: None,
        }
    }

    fn variable(path: PathBuf, weight: f32) -> Self {
        Self {
            path,
            index: 0,
            weight_axis: Some(weight),
        }
    }
}

/// The files of one font family, by weight. Only `regular` is required.
#[derive(Debug, Clone, PartialEq)]
pub struct FamilyFiles {
    /// What to call it in logs ("Segoe UI").
    pub name: &'static str,
    /// 400.
    pub regular: FaceFile,
    /// 500.
    pub medium: Option<FaceFile>,
    /// 600.
    pub semibold: Option<FaceFile>,
    /// 700.
    pub bold: Option<FaceFile>,
}

/// The operating system, for choosing where to look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Platform {
    /// Windows, with its fonts directory (`%WINDIR%\Fonts`).
    Windows {
        /// The fonts directory.
        fonts_dir: PathBuf,
    },
    /// macOS.
    MacOs,
    /// Linux and other Unix desktops.
    Linux,
}

impl Platform {
    /// The platform this binary runs on. On Windows the fonts directory is
    /// resolved from `%WINDIR%`, falling back to `C:\Windows`.
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::windows(std::env::var_os("WINDIR").map(PathBuf::from))
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }

    /// Windows, given the value of `%WINDIR%` (if set).
    pub fn windows(windir: Option<PathBuf>) -> Self {
        let windir = windir
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        Self::Windows {
            fonts_dir: windir.join("Fonts"),
        }
    }

    /// Candidate families, most wanted first.
    pub fn families(&self) -> Vec<FamilyFiles> {
        match self {
            Self::Windows { fonts_dir } => {
                let f = |name: &str| FaceFile::at(fonts_dir.join(name));
                vec![FamilyFiles {
                    name: "Segoe UI",
                    regular: f("segoeui.ttf"),
                    // Segoe UI has no 500; like a browser, 500 falls back to 400.
                    medium: None,
                    semibold: Some(f("seguisb.ttf")),
                    bold: Some(f("segoeuib.ttf")),
                }]
            }
            Self::MacOs => {
                let dir = Path::new("/System/Library/Fonts");
                // San Francisco ships as one variable font since macOS 10.15.
                let sf = dir.join("SFNS.ttf");
                let v = |weight| Some(FaceFile::variable(sf.clone(), weight));
                let text = |name: &str| FaceFile::at(dir.join(name));
                vec![
                    FamilyFiles {
                        name: "San Francisco",
                        regular: FaceFile::variable(sf.clone(), 400.0),
                        medium: v(510.0),
                        semibold: v(590.0),
                        bold: v(700.0),
                    },
                    // macOS 10.11 – 10.14.
                    FamilyFiles {
                        name: "San Francisco Text",
                        regular: text("SFNSText.ttf"),
                        medium: Some(text("SFNSText-Medium.ttf")),
                        semibold: Some(text("SFNSText-Semibold.ttf")),
                        bold: Some(text("SFNSText-Bold.ttf")),
                    },
                    FamilyFiles {
                        name: "Helvetica Neue",
                        regular: text("HelveticaNeue.ttc"),
                        medium: None,
                        semibold: None,
                        bold: None,
                    },
                ]
            }
            Self::Linux => {
                let f = |path: &str| FaceFile::at(PathBuf::from(path));
                let inter = |dir: &str, ext: &str| FamilyFiles {
                    name: "Inter",
                    regular: f(&format!("{dir}/Inter-Regular.{ext}")),
                    medium: Some(f(&format!("{dir}/Inter-Medium.{ext}"))),
                    semibold: Some(f(&format!("{dir}/Inter-SemiBold.{ext}"))),
                    bold: Some(f(&format!("{dir}/Inter-Bold.{ext}"))),
                };
                let noto = |dir: &str| FamilyFiles {
                    name: "Noto Sans",
                    regular: f(&format!("{dir}/NotoSans-Regular.ttf")),
                    medium: Some(f(&format!("{dir}/NotoSans-Medium.ttf"))),
                    semibold: Some(f(&format!("{dir}/NotoSans-SemiBold.ttf"))),
                    bold: Some(f(&format!("{dir}/NotoSans-Bold.ttf"))),
                };
                let dejavu = |dir: &str| FamilyFiles {
                    name: "DejaVu Sans",
                    regular: f(&format!("{dir}/DejaVuSans.ttf")),
                    medium: None,
                    semibold: None,
                    bold: Some(f(&format!("{dir}/DejaVuSans-Bold.ttf"))),
                };
                vec![
                    inter("/usr/share/fonts/opentype/inter", "otf"),
                    inter("/usr/share/fonts/truetype/inter", "ttf"),
                    inter("/usr/share/fonts/inter", "otf"),
                    noto("/usr/share/fonts/truetype/noto"),
                    noto("/usr/share/fonts/noto"),
                    noto("/usr/share/fonts/google-noto"),
                    dejavu("/usr/share/fonts/truetype/dejavu"),
                    dejavu("/usr/share/fonts/TTF"),
                    dejavu("/usr/share/fonts/dejavu-sans-fonts"),
                ]
            }
        }
    }
}

/// One face that loaded and parsed.
#[derive(Debug, Clone)]
pub struct LoadedFace {
    /// Where it came from.
    pub file: FaceFile,
    /// The bytes, ready for egui.
    pub data: Arc<FontData>,
}

/// The faces found for the strip and Settings: one family, four weights.
#[derive(Debug, Clone)]
pub struct SystemFaces {
    /// The family's name ("Segoe UI").
    pub name: &'static str,
    /// 400.
    pub regular: LoadedFace,
    /// 500, if the family has it.
    pub medium: Option<LoadedFace>,
    /// 600, if the family has it.
    pub semibold: Option<LoadedFace>,
    /// 700, if the family has it.
    pub bold: Option<LoadedFace>,
}

/// `true` when `bytes` hold a font egui can parse. egui panics on a font it
/// cannot parse, so every file is checked first, with the same parser.
pub fn parses(bytes: &[u8], index: u32) -> bool {
    skrifa::FontRef::from_index(bytes, index).is_ok()
}

fn load(source: &dyn FontSource, file: &FaceFile) -> Option<LoadedFace> {
    let bytes = source.read(&file.path)?;
    if !parses(&bytes, file.index) {
        tracing::debug!(path = %file.path.display(), "not a usable font");
        return None;
    }
    let mut data = FontData::from_owned(bytes);
    data.index = file.index;
    if let Some(weight) = file.weight_axis {
        data = data.tweak(FontTweak {
            coords: VariationCoords::new([(b"wght", weight)]),
            ..FontTweak::default()
        });
    }
    Some(LoadedFace {
        file: file.clone(),
        data: Arc::new(data),
    })
}

/// The first family in `families` whose regular face loads, with whichever
/// of its other weights load too. `None` when nothing does.
pub fn find(source: &dyn FontSource, families: &[FamilyFiles]) -> Option<SystemFaces> {
    families.iter().find_map(|family| {
        let regular = load(source, &family.regular)?;
        let other = |face: &Option<FaceFile>| face.as_ref().and_then(|f| load(source, f));
        Some(SystemFaces {
            name: family.name,
            regular,
            medium: other(&family.medium),
            semibold: other(&family.semibold),
            bold: other(&family.bold),
        })
    })
}

/// egui font definitions: the system faces (when found) ahead of egui's
/// bundled ones, which stay as the fallback for any glyph the system face
/// lacks. The weight families are always defined, so text set in them
/// renders even with no system font. A missing weight uses the nearest one
/// the family has (500 → 400; 600 → 700 → 400; 700 → 600 → 400).
pub fn definitions(faces: Option<&SystemFaces>) -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    let bundled = defs
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let chain = |name: Option<&str>| -> Vec<String> {
        name.map(str::to_owned)
            .into_iter()
            .chain(bundled.iter().cloned())
            .collect()
    };
    let register = |defs: &mut FontDefinitions, key: &str, face: Option<&LoadedFace>| {
        face.map(|face| {
            defs.font_data.insert(key.to_owned(), face.data.clone());
            key.to_owned()
        })
    };
    let (regular, medium, semibold, bold) = match faces {
        Some(faces) => {
            let regular = register(&mut defs, "system-regular", Some(&faces.regular));
            let medium = register(&mut defs, "system-medium", faces.medium.as_ref());
            let semibold = register(&mut defs, "system-semibold", faces.semibold.as_ref());
            let bold = register(&mut defs, "system-bold", faces.bold.as_ref());
            (regular, medium, semibold, bold)
        }
        None => (None, None, None, None),
    };
    let medium = medium.or_else(|| regular.clone());
    let semibold_or = semibold.clone().or_else(|| bold.clone());
    let bold = bold.or(semibold).or_else(|| regular.clone());
    let semibold = semibold_or.or_else(|| regular.clone());

    defs.families
        .insert(FontFamily::Proportional, chain(regular.as_deref()));
    defs.families
        .insert(FontFamily::Name(MEDIUM.into()), chain(medium.as_deref()));
    defs.families.insert(
        FontFamily::Name(SEMIBOLD.into()),
        chain(semibold.as_deref()),
    );
    defs.families
        .insert(FontFamily::Name(BOLD.into()), chain(bold.as_deref()));
    defs
}

/// Finds the system face for `platform` through `source` and installs it
/// (and the weight families) into `ctx`. Returns the family's name, or
/// `None` when egui's bundled font is used.
pub fn install(
    ctx: &egui::Context,
    source: &dyn FontSource,
    platform: &Platform,
) -> Option<&'static str> {
    let faces = find(source, &platform.families());
    match &faces {
        Some(faces) => tracing::info!(family = faces.name, "using the system font"),
        None => tracing::info!("no system font found; using the bundled font"),
    }
    ctx.set_fonts(definitions(faces.as_ref()));
    faces.map(|f| f.name)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// Real font bytes: egui's bundled face.
    fn font_bytes() -> Vec<u8> {
        FontDefinitions::default().font_data["Ubuntu-Light"]
            .bytes()
            .to_vec()
    }

    fn names(defs: &FontDefinitions, family: FontFamily) -> Vec<String> {
        defs.families[&family].clone()
    }

    #[test]
    fn windows_resolves_from_windir() {
        let platform = Platform::windows(Some(PathBuf::from("D:/Win")));
        assert_eq!(
            platform,
            Platform::Windows {
                fonts_dir: PathBuf::from("D:/Win").join("Fonts")
            }
        );
        let family = &platform.families()[0];
        assert_eq!(
            family.regular.path,
            PathBuf::from("D:/Win/Fonts/segoeui.ttf")
        );
        assert_eq!(
            family.semibold.as_ref().unwrap().path,
            PathBuf::from("D:/Win/Fonts/seguisb.ttf")
        );
        assert_eq!(
            family.bold.as_ref().unwrap().path,
            PathBuf::from("D:/Win/Fonts/segoeuib.ttf")
        );
        // No WINDIR, or an empty one: the usual place.
        for windir in [None, Some(PathBuf::new())] {
            assert_eq!(
                Platform::windows(windir),
                Platform::Windows {
                    fonts_dir: PathBuf::from(r"C:\Windows").join("Fonts")
                }
            );
        }
    }

    #[test]
    fn segoe_ui_loads_with_real_weights() {
        let fonts = PathBuf::from(r"C:\Windows").join("Fonts");
        let source = MemoryFontSource::new()
            .with(fonts.join("segoeui.ttf"), font_bytes())
            .with(fonts.join("seguisb.ttf"), font_bytes())
            .with(fonts.join("segoeuib.ttf"), font_bytes());
        let faces = find(&source, &Platform::windows(None).families()).unwrap();
        assert_eq!(faces.name, "Segoe UI");
        assert!(faces.medium.is_none());
        assert!(faces.semibold.is_some() && faces.bold.is_some());
        let defs = definitions(Some(&faces));
        assert_eq!(names(&defs, FontFamily::Proportional)[0], "system-regular");
        // 500 falls back to 400; 600 and 700 are their own faces.
        assert_eq!(
            names(&defs, FontFamily::Name(MEDIUM.into()))[0],
            "system-regular"
        );
        assert_eq!(
            names(&defs, FontFamily::Name(SEMIBOLD.into()))[0],
            "system-semibold"
        );
        assert_eq!(
            names(&defs, FontFamily::Name(BOLD.into()))[0],
            "system-bold"
        );
        // egui's own face stays as the glyph fallback.
        assert!(names(&defs, FontFamily::Proportional).contains(&"Ubuntu-Light".to_owned()));
    }

    #[test]
    fn macos_uses_the_variable_san_francisco_at_four_weights() {
        let source = MemoryFontSource::new().with("/System/Library/Fonts/SFNS.ttf", font_bytes());
        let faces = find(&source, &Platform::MacOs.families()).unwrap();
        assert_eq!(faces.name, "San Francisco");
        assert_eq!(faces.regular.file.weight_axis, Some(400.0));
        assert_eq!(
            faces.semibold.as_ref().unwrap().file.weight_axis,
            Some(590.0)
        );
        assert_eq!(faces.bold.as_ref().unwrap().file.weight_axis, Some(700.0));
        assert_eq!(
            faces
                .medium
                .as_ref()
                .unwrap()
                .data
                .tweak
                .coords
                .as_ref()
                .len(),
            1
        );
    }

    #[test]
    fn macos_falls_back_to_older_faces() {
        let source =
            MemoryFontSource::new().with("/System/Library/Fonts/HelveticaNeue.ttc", font_bytes());
        let faces = find(&source, &Platform::MacOs.families()).unwrap();
        assert_eq!(faces.name, "Helvetica Neue");
        let defs = definitions(Some(&faces));
        for family in [MEDIUM, SEMIBOLD, BOLD] {
            assert_eq!(
                names(&defs, FontFamily::Name(family.into()))[0],
                "system-regular"
            );
        }
    }

    #[test]
    fn linux_prefers_inter_then_noto_then_dejavu() {
        let families = Platform::Linux.families();
        let first: Vec<&str> = families.iter().map(|f| f.name).collect();
        assert_eq!(first[0], "Inter");
        assert!(first.contains(&"Noto Sans") && first.contains(&"DejaVu Sans"));

        let source = MemoryFontSource::new()
            .with(
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                font_bytes(),
            )
            .with(
                "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
                font_bytes(),
            )
            .with(
                "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
                font_bytes(),
            )
            .with(
                "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf",
                font_bytes(),
            );
        let faces = find(&source, &families).unwrap();
        assert_eq!(faces.name, "Noto Sans");
        let defs = definitions(Some(&faces));
        // No bold file: 700 uses the semibold face, not a faked one.
        assert_eq!(
            names(&defs, FontFamily::Name(BOLD.into()))[0],
            "system-semibold"
        );

        let dejavu = MemoryFontSource::new()
            .with(
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                font_bytes(),
            )
            .with(
                "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
                font_bytes(),
            );
        let defs = definitions(find(&dejavu, &families).as_ref());
        // No semibold: 600 uses the bold face.
        assert_eq!(
            names(&defs, FontFamily::Name(SEMIBOLD.into()))[0],
            "system-bold"
        );
    }

    #[test]
    fn missing_or_broken_files_fall_back_silently() {
        assert!(find(&MemoryFontSource::new(), &Platform::Linux.families()).is_none());
        let broken = MemoryFontSource::new().with(
            "/usr/share/fonts/opentype/inter/Inter-Regular.otf",
            b"not a font".to_vec(),
        );
        assert!(find(&broken, &Platform::Linux.families()).is_none());
        assert!(!parses(b"", 0));
        assert!(parses(&font_bytes(), 0));

        // Without system faces every family is still bound (to egui's own).
        let defs = definitions(None);
        for family in [MEDIUM, SEMIBOLD, BOLD] {
            assert_eq!(
                names(&defs, FontFamily::Name(family.into()))[0],
                "Ubuntu-Light"
            );
        }
        assert_eq!(names(&defs, FontFamily::Proportional)[0], "Ubuntu-Light");
    }

    #[test]
    fn install_sets_the_fonts_and_text_renders() {
        let ctx = egui::Context::default();
        let source = MemoryFontSource::new().with(
            "/usr/share/fonts/opentype/inter/Inter-Regular.otf",
            font_bytes(),
        );
        assert_eq!(install(&ctx, &source, &Platform::Linux), Some("Inter"));
        // Text in every weight lays out (the families exist).
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                for family in [MEDIUM, SEMIBOLD, BOLD] {
                    ui.label(
                        egui::RichText::new("Builds · Failed")
                            .family(FontFamily::Name(family.into())),
                    );
                }
            });
            output.textures_delta.clear();
        }
        assert_eq!(
            install(&ctx, &MemoryFontSource::new(), &Platform::Linux),
            None
        );
        assert!(SystemFontSource
            .read(Path::new("/definitely/not/here.ttf"))
            .is_none());
        assert!(!Platform::current().families().is_empty());
    }
}
