//! Font files that template text draws in: the bundled OFL fonts and a brand's own files. A
//! [`Face`] names a file by its content, so every machine draws the same glyphs, unlike the
//! system fonts that [`crate::drawing::Font::Sans`] and `Mono` use.
//!
//! Files register once per process; the vector pass hands each one to parley on first use.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use shot_core::BundledFont;

/// A registered font file at a weight. Variable fonts take the weight on their `wght` axis,
/// a static file draws at its own weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Face {
    id: u64,
    pub weight: u16,
}

/// A registered file: its bytes and its blake3 hash.
#[derive(Clone)]
pub(crate) struct FaceFile {
    pub bytes: Arc<Vec<u8>>,
    pub hash: [u8; 32],
}

fn registry() -> &'static Mutex<HashMap<u64, FaceFile>> {
    static FACES: OnceLock<Mutex<HashMap<u64, FaceFile>>> = OnceLock::new();
    FACES.get_or_init(Mutex::default)
}

impl Face {
    /// Registers a font file and returns it at weight 400.
    pub fn load(bytes: Arc<Vec<u8>>) -> Self {
        let hash = *blake3::hash(&bytes).as_bytes();
        let id = u64::from_le_bytes(hash[..8].try_into().expect("8 bytes"));
        registry()
            .lock()
            .expect("font registry poisoned")
            .entry(id)
            .or_insert(FaceFile { bytes, hash });
        Self { id, weight: 400 }
    }

    /// A bundled font at `weight`.
    pub fn bundled(font: BundledFont, weight: u16) -> Self {
        static BUNDLED: OnceLock<[Face; 4]> = OnceLock::new();
        let faces = BUNDLED.get_or_init(|| {
            let load = |bytes: &'static [u8]| Face::load(Arc::new(bytes.to_vec()));
            [
                load(include_bytes!("../assets/fonts/Inter.ttf")),
                load(include_bytes!("../assets/fonts/Geist.ttf")),
                load(include_bytes!("../assets/fonts/GeistMono.ttf")),
                load(include_bytes!("../assets/fonts/JetBrainsMono.ttf")),
            ]
        });
        let face = match font {
            BundledFont::Inter => faces[0],
            BundledFont::Geist => faces[1],
            BundledFont::GeistMono => faces[2],
            BundledFont::JetbrainsMono => faces[3],
        };
        face.at(weight)
    }

    pub fn at(self, weight: u16) -> Self {
        Self { weight, ..self }
    }

    /// The family name the file registers under: unique, so no installed font can stand in.
    pub(crate) fn family(self) -> String {
        format!("shot-face-{:016x}", self.id)
    }

    pub(crate) fn file(self) -> FaceFile {
        registry()
            .lock()
            .expect("font registry poisoned")
            .get(&self.id)
            .cloned()
            .expect("faces are registered when made")
    }

    /// blake3 of the file, for chunk keys.
    pub fn hash(self) -> [u8; 32] {
        self.file().hash
    }
}
