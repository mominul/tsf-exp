use std::{env, fs, path::PathBuf, sync::OnceLock};

use riti::config::Config;
use serde::Deserialize;

use crate::{DEFAULT_CONF, Error, IME_NAME, Result, extend::ResultExt};

// use parking_lot::{RwLock, RwLockReadGuard};
//
// static CONF: OnceLock<RwLock<Conf>> = OnceLock::new();
//
// pub fn get() -> RwLockReadGuard<'static, Conf> {
//     CONF2.get_or_init(||RwLock::new(Conf::open_or_default())).read_recursive()
// }
//
// pub fn reload() {
//     // todo check for last modified
//     let mut conf = CONF2.get().unwrap().write();
//     *conf = Conf::open_or_default();
// }

static CONF: OnceLock<Conf> = OnceLock::new();

pub fn get() -> &'static Conf {
    //log::info!("[{}:{};{}] {}()", file!(), line!(), column!(), crate::function!());

    CONF.get_or_init(Conf::open_or_default)
}

#[derive(Deserialize, Debug)]
pub struct Conf {
    pub font: Font,
    pub layout: Layout,
    pub color: Color,
    pub behavior: Behavior,
}

impl Default for Conf {
    fn default() -> Self {
        //log::info!("[{}:{};{}] {}()", file!(), line!(), column!(), crate::function!());

        toml::from_str(DEFAULT_CONF).unwrap()
    }
}

impl Conf {
    pub fn open() -> Result<Conf> {
        //log::info!("[{}:{};{}] {}()", file!(), line!(), column!(), crate::function!());

        let path = PathBuf::from(env::var("APPDATA")?)
            .join(IME_NAME)
            .join("conf.toml");
        if !path.exists() {
            fs::create_dir_all(path.parent().unwrap())?;
            fs::write(path, DEFAULT_CONF)?;
            return Ok(Conf::default());
        }
        let conf = fs::read_to_string(path)?;
        let conf = toml::from_str(&conf).map_err(|e| Error::ParseError("conf.toml", e))?;
        Ok(conf)
    }

    pub fn open_or_default() -> Conf {
        //log::info!("[{}:{};{}] {}()", file!(), line!(), column!(), crate::function!());

        Conf::open().log_err().unwrap_or_default()
    }
}

#[derive(Deserialize, Debug)]
pub struct Font {
    pub name: String,
    pub size: i32,
}

#[derive(Deserialize, Debug)]
pub struct Color {
    pub candidate: csscolorparser::Color,
    pub index: csscolorparser::Color,
    pub background: csscolorparser::Color,
    pub clip: csscolorparser::Color,
    pub highlight: csscolorparser::Color,
    pub highlighted: csscolorparser::Color,
}

#[derive(Deserialize, Debug)]
pub struct Layout {
    pub vertical: bool,
}

#[derive(Deserialize, Debug)]
pub struct Behavior {
    pub toggle: Option<Toggle>,
    pub long_pi: bool,
    pub long_glyph: bool,
}

#[derive(Deserialize, Debug, Clone, Copy)]
pub enum Toggle {
    #[serde(alias = "英数")]
    Eisu,
    Ctrl,
    CapsLock,
}

use winreg::enums::*;
use winreg::RegKey;

/// Settings manager for OpenBangla Keyboard
/// Reads settings from Windows Registry (QSettings format)
pub struct Settings {
    base_key: RegKey,
}

impl Settings {
    /// Creates a new Settings instance, creating the key if it doesn't exist
    pub fn load_or_create() -> Result<Self> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (base_key, _) = hkcu.create_subkey(r"Software\OpenBangla\Keyboard")?;
        Ok(Self { base_key })
    }

    // Helper methods for reading values
    fn get_string_from_key(&self, key: &RegKey, name: &str) -> Option<String> {
        key.get_value::<String, _>(name).ok()
    }

    fn get_bool(&self, subkey: &str, name: &str, default: bool) -> bool {
        self.base_key
            .open_subkey(subkey)
            .ok()
            .and_then(|key| self.get_string_from_key(&key, name))
            .map(|v| v == "true")
            .unwrap_or(default)
    }

    fn get_string(&self, subkey: &str, name: &str, default: &str) -> String {
        self.base_key
            .open_subkey(subkey)
            .ok()
            .and_then(|key| self.get_string_from_key(&key, name))
            .unwrap_or_else(|| default.to_string())
    }

    pub fn get_enter_key_closes_prev_win(&self) -> bool {
        // self.get_bool_direct("EnterKeyClosesPrevWin", false)
        self.get_bool(r"settings", "EnterKeyClosesPrevWin", false)
    }

    pub fn get_ansi_encoding(&self) -> bool {
        // self.get_bool_direct("ANSI", false)
        self.get_bool(r"settings", "ANSI", false)
    }

    pub fn get_smart_quoting(&self) -> bool {
        // self.get_bool_direct("SmartQuoting", true)
        self.get_bool(r"settings", "SmartQuoting", true)
    }

    // Layout settings
    pub fn get_layout_path(&self) -> String {
        self.get_string("layout", "path", "avro_phonetic")
    }

    // Fixed Layout settings
    pub fn get_show_prev_win_fixed(&self) -> bool {
        self.get_bool(r"settings\FixedLayout", "ShowPrevWin", true)
    }

    pub fn get_auto_vowel_form_fixed(&self) -> bool {
        self.get_bool(r"settings\FixedLayout", "AutoVowelForm", true)
    }

    pub fn get_auto_chandra_pos_fixed(&self) -> bool {
        self.get_bool(r"settings\FixedLayout", "AutoChandraPos", true)
    }

    pub fn get_traditional_kar_fixed(&self) -> bool {
        self.get_bool(r"settings\FixedLayout", "TraditionalKar", false)
    }

    pub fn get_number_pad_fixed(&self) -> bool {
        self.get_bool(r"settings\FixedLayout", "NumberPad", true)
    }

    pub fn get_old_reph(&self) -> bool {
        self.get_bool(r"settings\FixedLayout", "OldReph", true)
    }

    pub fn get_fixed_old_kar_order(&self) -> bool {
        self.get_bool(r"settings\FixedLayout", "OldKarOrder", false)
    }

    // Candidate Window settings
    pub fn get_candidate_win_horizontal(&self) -> bool {
        self.get_bool(r"settings\CandidateWin", "Horizontal", true)
    }

    pub fn get_show_cw_phonetic(&self) -> bool {
        self.get_bool(r"settings\CandidateWin", "Phonetic", true)
    }

    // Preview Window settings
    pub fn get_suggestion_include_english(&self) -> bool {
        self.get_bool(r"settings\PreviewWin", "IncludeEnglish", true)
    }
}

pub fn load_riti_config() -> Config {
    let Ok(settings) = Settings::load_or_create() else {
        log::error!("Failed to load settings from registry. Using default Riti config.");
        return Config::default();
    };

    let mut config = Config::default();
    config.set_layout_file_path(&settings.get_layout_path());
    config.set_database_dir("");
    config.set_phonetic_suggestion(settings.get_show_cw_phonetic());
    config.set_suggestion_include_english(settings.get_suggestion_include_english());

    config.set_fixed_suggestion(settings.get_show_prev_win_fixed());
    config.set_fixed_automatic_vowel(settings.get_auto_vowel_form_fixed());
    config.set_fixed_automatic_chandra(settings.get_auto_chandra_pos_fixed());
    config.set_fixed_traditional_kar(settings.get_traditional_kar_fixed());
    config.set_fixed_numpad(settings.get_number_pad_fixed());
    config.set_fixed_old_reph(settings.get_old_reph());
    config.set_fixed_old_kar_order(settings.get_fixed_old_kar_order());

    config.set_ansi_encoding(settings.get_ansi_encoding());
    config.set_smart_quote(settings.get_smart_quoting());

    config
}

#[test]
fn test_open() {
    //log::info!("[{}:{};{}] {}()", file!(), line!(), column!(), crate::function!());

    let conf = get();
    println!("{conf:#?}")
}
