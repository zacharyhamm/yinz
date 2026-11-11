use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use crate::text::font::FaceRequest;
#[cfg(test)]
use crate::text::font::FaceStyle;

#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
mod scan;

#[cfg(not(any(target_os = "macos", windows)))]
use fontconfig::Fontconfig as Backend;
#[cfg(any(target_os = "macos", windows))]
use scan::Scanner as Backend;

static BACKEND: OnceLock<Option<Mutex<Backend>>> = OnceLock::new();

#[derive(Clone, Copy)]
pub struct Discovery(&'static Mutex<Backend>);

impl Discovery {
    pub fn open() -> Option<Self> {
        BACKEND.get_or_init(Backend::load).as_ref().map(Discovery)
    }

    pub fn match_codepoint(
        &self,
        request: impl Into<FaceRequest>,
        character: char,
    ) -> Option<(PathBuf, u32)> {
        let request = request.into();
        match self.0.lock() {
            Ok(backend) => backend.match_codepoint(request, character),
            Err(_) => {
                report!("the font discovery mutex was poisoned");
                None
            },
        }
    }

    pub fn match_family(
        &self,
        family: &str,
        request: impl Into<FaceRequest>,
    ) -> Option<(PathBuf, u32)> {
        let request = request.into();
        match self.0.lock() {
            Ok(backend) => backend.match_family(family, request),
            Err(_) => {
                report!("the font discovery mutex was poisoned");
                None
            },
        }
    }

    pub fn match_emoji(&self, character: char) -> Option<(PathBuf, u32)> {
        match self.0.lock() {
            Ok(backend) => backend.match_emoji(character),
            Err(_) => {
                report!("the font discovery mutex was poisoned");
                None
            },
        }
    }

    pub fn monospace_families(&self) -> Vec<String> {
        match self.0.lock() {
            Ok(backend) => backend.monospace_families(),
            Err(_) => {
                report!("the font discovery mutex was poisoned");
                Vec::new()
            },
        }
    }

    #[cfg(test)]
    pub fn synthetic(directory: &std::path::Path) -> Option<Self> {
        Backend::synthetic(directory)
            .map(|backend| Discovery(Box::leak(Box::new(Mutex::new(backend)))))
    }
}

// fontconfig is opened at runtime rather than linked, so the terminal still starts (on its
// embedded fonts) where libfontconfig is missing. That is why this module calls C through
// raw function pointers and needs `unsafe` throughout.
#[cfg(not(any(target_os = "macos", windows)))]
mod fontconfig {
    use std::ffi::{c_char, c_int, c_void, CStr, CString};
    use std::path::PathBuf;
    use std::sync::Mutex;

    use crate::text::font::FaceRequest;

    fn library_name() -> Option<&'static str> {
        cfg!(target_os = "linux").then_some("libfontconfig.so.1")
    }

    const FC_MATCH_PATTERN: c_int = 0;
    const FC_RESULT_MATCH: c_int = 0;
    const FC_WEIGHTS: [(u16, c_int); 11] = [
        (100, 0),
        (200, 40),
        (300, 50),
        (350, 55),
        (380, 75),
        (400, 80),
        (500, 100),
        (600, 180),
        (700, 200),
        (800, 205),
        (900, 210),
    ];

    pub(super) fn fontconfig_weight(weight: u16) -> c_int {
        let weight = weight.clamp(FC_WEIGHTS[0].0, FC_WEIGHTS[FC_WEIGHTS.len() - 1].0);
        for pair in FC_WEIGHTS.windows(2) {
            let ((low, low_fc), (high, high_fc)) = (pair[0], pair[1]);
            if weight <= high {
                let span = (high - low) as f32;
                let along = (weight - low) as f32 / span;
                return (low_fc as f32 + along * (high_fc - low_fc) as f32).round() as c_int;
            }
        }
        FC_WEIGHTS[FC_WEIGHTS.len() - 1].1
    }
    const FC_SLANT_ROMAN: c_int = 0;
    const FC_SLANT_ITALIC: c_int = 100;

    const FAMILY: &[u8] = b"family\0";
    const WEIGHT: &[u8] = b"weight\0";
    const SLANT: &[u8] = b"slant\0";
    const CHARSET: &[u8] = b"charset\0";
    const FILE: &[u8] = b"file\0";
    const INDEX: &[u8] = b"index\0";
    const MONOSPACE: &[u8] = b"monospace\0";
    const EMOJI: &[u8] = b"emoji\0";
    const COLOR: &[u8] = b"color\0";
    const FC_TRUE: c_int = 1;
    const SPACING: &[u8] = b"spacing\0";
    const FC_MONO: c_int = 100;

    const MAX_FAMILY_NAMES: c_int = 8;

    type Config = c_void;
    type Pattern = c_void;
    type CharSet = c_void;
    type ObjectSet = c_void;

    #[repr(C)]
    struct FontSet {
        nfont: c_int,
        sfont: c_int,
        fonts: *mut *mut Pattern,
    }

    struct Api {
        init_load_config_and_fonts: unsafe extern "C" fn() -> *mut Config,
        pattern_create: unsafe extern "C" fn() -> *mut Pattern,
        pattern_destroy: unsafe extern "C" fn(*mut Pattern),
        pattern_add_string: unsafe extern "C" fn(*mut Pattern, *const c_char, *const u8) -> c_int,
        pattern_add_integer: unsafe extern "C" fn(*mut Pattern, *const c_char, c_int) -> c_int,
        pattern_add_bool: unsafe extern "C" fn(*mut Pattern, *const c_char, c_int) -> c_int,
        pattern_add_charset:
            unsafe extern "C" fn(*mut Pattern, *const c_char, *const CharSet) -> c_int,
        pattern_get_string:
            unsafe extern "C" fn(*const Pattern, *const c_char, c_int, *mut *mut u8) -> c_int,
        pattern_get_integer:
            unsafe extern "C" fn(*const Pattern, *const c_char, c_int, *mut c_int) -> c_int,
        pattern_get_charset:
            unsafe extern "C" fn(*const Pattern, *const c_char, c_int, *mut *mut CharSet) -> c_int,
        charset_create: unsafe extern "C" fn() -> *mut CharSet,
        charset_destroy: unsafe extern "C" fn(*mut CharSet),
        charset_add_char: unsafe extern "C" fn(*mut CharSet, u32) -> c_int,
        charset_has_char: unsafe extern "C" fn(*const CharSet, u32) -> c_int,
        config_substitute: unsafe extern "C" fn(*mut Config, *mut Pattern, c_int) -> c_int,
        default_substitute: unsafe extern "C" fn(*mut Pattern),
        font_match: unsafe extern "C" fn(*mut Config, *mut Pattern, *mut c_int) -> *mut Pattern,
        object_set_create: unsafe extern "C" fn() -> *mut ObjectSet,
        object_set_add: unsafe extern "C" fn(*mut ObjectSet, *const c_char) -> c_int,
        object_set_destroy: unsafe extern "C" fn(*mut ObjectSet),
        font_list: unsafe extern "C" fn(*mut Config, *mut Pattern, *mut ObjectSet) -> *mut FontSet,
        font_set_destroy: unsafe extern "C" fn(*mut FontSet),
        _library: libloading::Library,
    }

    pub(super) struct Fontconfig {
        api: Api,
        config: *mut Config,
    }

    unsafe impl Send for Fontconfig {}

    impl Fontconfig {
        pub(super) fn load() -> Option<Mutex<Self>> {
            let Some(library) = library_name() else {
                report!(
                    "system font discovery has no backend on this platform; \
                     falling back to the embedded fonts"
                );
                return None;
            };
            Self::load_from(library)
        }

        pub(super) fn load_from(library: &str) -> Option<Mutex<Self>> {
            let api = match unsafe { load(library) } {
                Ok(api) => api,
                Err(e) => {
                    report!(
                        "system font discovery is unavailable ({}); \
                         falling back to the embedded fonts",
                        e
                    );
                    return None;
                },
            };

            let config = unsafe { (api.init_load_config_and_fonts)() };
            if config.is_null() {
                report!("fontconfig produced no configuration");
                return None;
            }

            Some(Mutex::new(Self { api, config }))
        }

        #[cfg(test)]
        pub(super) fn synthetic(directory: &std::path::Path) -> Option<Self> {
            type ConfigCreate = unsafe extern "C" fn() -> *mut Config;
            type AddFontDir = unsafe extern "C" fn(*mut Config, *const u8) -> c_int;
            let api = unsafe { load(library_name()?) }.ok()?;
            let extra = unsafe { libloading::Library::new(library_name()?) }.ok()?;
            let config_create: ConfigCreate = *unsafe { extra.get(b"FcConfigCreate\0") }.ok()?;
            let add_font_dir: AddFontDir =
                *unsafe { extra.get(b"FcConfigAppFontAddDir\0") }.ok()?;
            let config = unsafe { config_create() };
            if config.is_null() {
                return None;
            }
            let directory = CString::new(directory.to_str()?).ok()?;
            let added = unsafe { add_font_dir(config, directory.as_ptr() as *const u8) };
            (added != 0).then_some(Self { api, config })
        }

        pub(super) fn match_codepoint(
            &self,
            request: FaceRequest,
            character: char,
        ) -> Option<(PathBuf, u32)> {
            let api = &self.api;
            unsafe {
                let charset = (api.charset_create)();
                if charset.is_null() {
                    return None;
                }
                (api.charset_add_char)(charset, character as u32);

                let pattern = (api.pattern_create)();
                if pattern.is_null() {
                    (api.charset_destroy)(charset);
                    return None;
                }
                (api.pattern_add_string)(
                    pattern,
                    FAMILY.as_ptr() as *const c_char,
                    MONOSPACE.as_ptr(),
                );
                (api.pattern_add_integer)(
                    pattern,
                    WEIGHT.as_ptr() as *const c_char,
                    fontconfig_weight(request.weight),
                );
                (api.pattern_add_integer)(
                    pattern,
                    SLANT.as_ptr() as *const c_char,
                    if request.italic {
                        FC_SLANT_ITALIC
                    } else {
                        FC_SLANT_ROMAN
                    },
                );
                (api.pattern_add_charset)(pattern, CHARSET.as_ptr() as *const c_char, charset);
                (api.config_substitute)(self.config, pattern, FC_MATCH_PATTERN);
                (api.default_substitute)(pattern);

                let mut result: c_int = 0;
                let matched = (api.font_match)(self.config, pattern, &mut result);
                let found = if matched.is_null() || result != FC_RESULT_MATCH {
                    None
                } else {
                    self.describe(matched, character)
                };

                if !matched.is_null() {
                    (api.pattern_destroy)(matched);
                }
                (api.pattern_destroy)(pattern);
                (api.charset_destroy)(charset);
                found
            }
        }

        pub(super) fn match_emoji(&self, character: char) -> Option<(PathBuf, u32)> {
            let api = &self.api;
            unsafe {
                let charset = (api.charset_create)();
                if charset.is_null() {
                    return None;
                }
                (api.charset_add_char)(charset, character as u32);

                let pattern = (api.pattern_create)();
                if pattern.is_null() {
                    (api.charset_destroy)(charset);
                    return None;
                }
                (api.pattern_add_string)(pattern, FAMILY.as_ptr() as *const c_char, EMOJI.as_ptr());
                (api.pattern_add_bool)(pattern, COLOR.as_ptr() as *const c_char, FC_TRUE);
                (api.pattern_add_charset)(pattern, CHARSET.as_ptr() as *const c_char, charset);
                (api.config_substitute)(self.config, pattern, FC_MATCH_PATTERN);
                (api.default_substitute)(pattern);

                let mut result: c_int = 0;
                let matched = (api.font_match)(self.config, pattern, &mut result);
                let found = if matched.is_null() || result != FC_RESULT_MATCH {
                    None
                } else {
                    self.describe(matched, character)
                };

                if !matched.is_null() {
                    (api.pattern_destroy)(matched);
                }
                (api.pattern_destroy)(pattern);
                (api.charset_destroy)(charset);
                found
            }
        }

        pub(super) fn match_family(
            &self,
            family: &str,
            request: FaceRequest,
        ) -> Option<(PathBuf, u32)> {
            let api = &self.api;
            let requested = CString::new(family).ok()?;
            unsafe {
                let pattern = (api.pattern_create)();
                if pattern.is_null() {
                    return None;
                }
                (api.pattern_add_string)(
                    pattern,
                    FAMILY.as_ptr() as *const c_char,
                    requested.as_ptr() as *const u8,
                );
                (api.pattern_add_integer)(
                    pattern,
                    WEIGHT.as_ptr() as *const c_char,
                    fontconfig_weight(request.weight),
                );
                (api.pattern_add_integer)(
                    pattern,
                    SLANT.as_ptr() as *const c_char,
                    if request.italic {
                        FC_SLANT_ITALIC
                    } else {
                        FC_SLANT_ROMAN
                    },
                );
                (api.config_substitute)(self.config, pattern, FC_MATCH_PATTERN);
                (api.default_substitute)(pattern);

                let mut result: c_int = 0;
                let matched = (api.font_match)(self.config, pattern, &mut result);
                let found = if matched.is_null()
                    || result != FC_RESULT_MATCH
                    || !self.answers_to(matched, family)
                {
                    None
                } else {
                    self.locate(matched)
                };

                if !matched.is_null() {
                    (api.pattern_destroy)(matched);
                }
                (api.pattern_destroy)(pattern);
                found
            }
        }

        pub(super) fn monospace_families(&self) -> Vec<String> {
            let api = &self.api;
            let mut families = Vec::new();
            unsafe {
                let pattern = (api.pattern_create)();
                if pattern.is_null() {
                    return families;
                }
                (api.pattern_add_integer)(pattern, SPACING.as_ptr() as *const c_char, FC_MONO);
                let objects = (api.object_set_create)();
                if objects.is_null() {
                    (api.pattern_destroy)(pattern);
                    return families;
                }
                (api.object_set_add)(objects, FAMILY.as_ptr() as *const c_char);
                let listed = (api.font_list)(self.config, pattern, objects);
                if !listed.is_null() {
                    let set = &*listed;
                    for position in 0..set.nfont.max(0) as usize {
                        let font = *set.fonts.add(position);
                        let mut name: *mut u8 = std::ptr::null_mut();
                        if !font.is_null()
                            && (api.pattern_get_string)(
                                font,
                                FAMILY.as_ptr() as *const c_char,
                                0,
                                &mut name,
                            ) == FC_RESULT_MATCH
                            && !name.is_null()
                            && let Ok(name) = CStr::from_ptr(name as *const c_char).to_str()
                        {
                            families.push(name.to_owned());
                        }
                    }
                    (api.font_set_destroy)(listed);
                }
                (api.object_set_destroy)(objects);
                (api.pattern_destroy)(pattern);
            }
            super::scan::sort_families(&mut families);
            families
        }

        unsafe fn answers_to(&self, matched: *mut Pattern, requested: &str) -> bool {
            unsafe {
                let api = &self.api;
                for index in 0..MAX_FAMILY_NAMES {
                    let mut name: *mut u8 = std::ptr::null_mut();
                    if (api.pattern_get_string)(
                        matched,
                        FAMILY.as_ptr() as *const c_char,
                        index,
                        &mut name,
                    ) != FC_RESULT_MATCH
                        || name.is_null()
                    {
                        return false;
                    }
                    if CStr::from_ptr(name as *const c_char)
                        .to_str()
                        .is_ok_and(|name| name.eq_ignore_ascii_case(requested))
                    {
                        return true;
                    }
                }
                false
            }
        }

        unsafe fn describe(
            &self,
            matched: *mut Pattern,
            character: char,
        ) -> Option<(PathBuf, u32)> {
            unsafe {
                let api = &self.api;

                let mut covered: *mut CharSet = std::ptr::null_mut();
                if (api.pattern_get_charset)(
                    matched,
                    CHARSET.as_ptr() as *const c_char,
                    0,
                    &mut covered,
                ) == FC_RESULT_MATCH
                    && !covered.is_null()
                    && (api.charset_has_char)(covered, character as u32) == 0
                {
                    return None;
                }

                self.locate(matched)
            }
        }

        unsafe fn locate(&self, matched: *mut Pattern) -> Option<(PathBuf, u32)> {
            unsafe {
                let api = &self.api;

                let mut path: *mut u8 = std::ptr::null_mut();
                if (api.pattern_get_string)(matched, FILE.as_ptr() as *const c_char, 0, &mut path)
                    != FC_RESULT_MATCH
                    || path.is_null()
                {
                    return None;
                }
                let path = CStr::from_ptr(path as *const c_char).to_str().ok()?;

                let mut index: c_int = 0;
                if (api.pattern_get_integer)(
                    matched,
                    INDEX.as_ptr() as *const c_char,
                    0,
                    &mut index,
                ) != FC_RESULT_MATCH
                {
                    index = 0;
                }

                Some((PathBuf::from(path), face_index(index)))
            }
        }
    }

    /// fontconfig packs a variable font's named instance into the upper 16
    /// bits of FC_INDEX (`instance << 16 | face`). Only the lower 16 bits
    /// address a face in the file; the weight is applied separately through
    /// the variation axes, so the instance number is dropped here.
    pub(super) fn face_index(index: c_int) -> u32 {
        (index as u32) & 0xffff
    }

    unsafe fn load(name: &str) -> Result<Api, libloading::Error> {
        unsafe {
            let library = libloading::Library::new(name)?;

            macro_rules! symbol {
                ($name:literal) => {
                    *library.get($name)?
                };
            }

            Ok(Api {
                init_load_config_and_fonts: symbol!(b"FcInitLoadConfigAndFonts\0"),
                pattern_create: symbol!(b"FcPatternCreate\0"),
                pattern_destroy: symbol!(b"FcPatternDestroy\0"),
                pattern_add_string: symbol!(b"FcPatternAddString\0"),
                pattern_add_integer: symbol!(b"FcPatternAddInteger\0"),
                pattern_add_bool: symbol!(b"FcPatternAddBool\0"),
                pattern_add_charset: symbol!(b"FcPatternAddCharSet\0"),
                pattern_get_string: symbol!(b"FcPatternGetString\0"),
                pattern_get_integer: symbol!(b"FcPatternGetInteger\0"),
                pattern_get_charset: symbol!(b"FcPatternGetCharSet\0"),
                charset_create: symbol!(b"FcCharSetCreate\0"),
                charset_destroy: symbol!(b"FcCharSetDestroy\0"),
                charset_add_char: symbol!(b"FcCharSetAddChar\0"),
                charset_has_char: symbol!(b"FcCharSetHasChar\0"),
                config_substitute: symbol!(b"FcConfigSubstitute\0"),
                default_substitute: symbol!(b"FcDefaultSubstitute\0"),
                font_match: symbol!(b"FcFontMatch\0"),
                object_set_create: symbol!(b"FcObjectSetCreate\0"),
                object_set_add: symbol!(b"FcObjectSetAdd\0"),
                object_set_destroy: symbol!(b"FcObjectSetDestroy\0"),
                font_list: symbol!(b"FcFontList\0"),
                font_set_destroy: symbol!(b"FcFontSetDestroy\0"),
                _library: library,
            })
        }
    }
}

#[cfg(test)]
#[path = "unit/discovery_tests.rs"]
mod tests;
