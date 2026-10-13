use std::{
    ffi::c_void,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use shikifsrs_elden_ring::{
    TaskPhase, Ww171File,
    character_equipment::WeaponSelectionMode,
    current_sound_conditions::SoundConditionReadConfig,
    sound_bank_resident::{ResidentSoundBankConfig, ResidentSoundBankSlot},
    sound_bank_schedule::{BankAssetSource, SoundBankScheduleConfig},
    sound_conditions::ConditionBankMapping as SdkMapping,
};
use shikifsrs_runtime::memory::CurrentProcess;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use windows::Win32::{
    Foundation::HMODULE,
    System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleA},
};
use windows::core::BOOL;

#[cfg(test)]
use std::collections::HashSet;

mod sfx;

static SOUND: ResidentSoundBankSlot = ResidentSoundBankSlot::new();
static STARTED: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
const AK_SUCCESS: i32 = 1;
#[cfg(test)]
const AK_BANK_ALREADY_LOADED: i32 = 69;
#[derive(Clone)]
struct Config {
    sfx: sfx::Settings,
    enabled: bool,
    log_enabled: bool,
    sound_bank_ids: Vec<u32>,
    start_delay_ms: u64,
    retries_per_id: u32,
    retry_every_frames: u32,
    memory_copy_enabled: bool,
    bank_file_dirs: Vec<PathBuf>,
    conditional_enabled: bool,
    condition_hand: HandMode,
    condition_bank_mappings: Vec<ConditionBankMapping>,
    sp_effect_bank_mappings: Vec<ConditionBankMapping>,
    poll_every_frames: u32,
    unload_when_inactive: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sfx: sfx::Settings::default(),
            enabled: true,
            log_enabled: true,
            sound_bank_ids: Vec::new(),
            start_delay_ms: 5000,
            retries_per_id: 120,
            retry_every_frames: 60,
            memory_copy_enabled: true,
            bank_file_dirs: vec![PathBuf::from("auto")],
            conditional_enabled: false,
            condition_hand: HandMode::Auto,
            condition_bank_mappings: Vec::new(),
            sp_effect_bank_mappings: Vec::new(),
            poll_every_frames: 6,
            unload_when_inactive: true,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ConditionBankMapping {
    condition_id: i32,
    bank_id: u32,
}

#[derive(Clone, Copy, Debug)]
enum HandMode {
    Auto,
    Right,
    Left,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BankOwnership {
    Loader,
    External,
}
/// # Safety
///
/// This is the Windows DLL entry point. It is called by the loader.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(
    hmodule: HMODULE,
    reason: u32,
    _reserved: *mut c_void,
) -> BOOL {
    if reason != 1 || STARTED.swap(true, Ordering::AcqRel) {
        return BOOL(1);
    }
    let hmodule = hmodule.0 as usize;

    std::thread::spawn(move || {
        let config = Config::load(hmodule);
        let log_path = if config.log_enabled {
            sidecar_path(hmodule, "ersoundbank_loader.log")
        } else {
            None
        };

        append_log(
            &log_path,
            &format!(
                "loaded enabled={} log_enabled={} sound_bank_ids={:?} start_delay_ms={} retries_per_id={} retry_every_frames={} memory_copy_enabled={} bank_file_dirs={:?} conditional_enabled={} condition_hand={:?} condition_bank_mappings={} sp_effect_bank_mappings={} poll_every_frames={} unload_when_inactive={} sfx={}",
                config.enabled,
                config.log_enabled,
                config.sound_bank_ids,
                config.start_delay_ms,
                config.retries_per_id,
                config.retry_every_frames,
                config.memory_copy_enabled,
                config.bank_file_dirs,
                config.conditional_enabled,
                config.condition_hand,
                format_condition_bank_mappings(&config.condition_bank_mappings),
                format_condition_bank_mappings(&config.sp_effect_bank_mappings),
                config.poll_every_frames,
                config.unload_when_inactive,
                config.sfx.summary(),
            ),
        );

        if !config.enabled {
            append_log(&log_path, "loader disabled by config");
            return;
        }

        if !config.conditional_enabled && config.sound_bank_ids.is_empty() && !config.sfx.enabled {
            append_log(&log_path, "no sound_bank_ids configured");
            return;
        }

        start_loader_after_delay(log_path, config);
    });

    BOOL(1)
}

impl Config {
    fn load(hmodule: usize) -> Self {
        let mut config = Self::default();
        let mut sound_bank_ids_configured = false;
        let mut bank_file_dirs_configured = false;
        let mut condition_mappings_configured = false;
        let mut sp_effect_mappings_configured = false;

        let Some(path) = sidecar_path(hmodule, "ersoundbank_loader.ini") else {
            return config;
        };
        let Ok(contents) = fs::read_to_string(path) else {
            return config;
        };

        for raw_line in contents.lines() {
            let line = raw_line
                .split_once('#')
                .map_or(raw_line, |(value, _)| value)
                .trim();
            if line.is_empty() {
                continue;
            }

            let Some((key, value)) = line.split_once('=') else {
                continue;
            };

            match (key.trim(), value.trim()) {
                ("enabled", value) => {
                    if let Some(parsed) = parse_bool(value) {
                        config.enabled = parsed;
                    }
                }
                ("log_enabled", value) | ("enable_log", value) | ("write_log", value) => {
                    if let Some(parsed) = parse_bool(value) {
                        config.log_enabled = parsed;
                    }
                }
                ("sound_bank_ids", value)
                | ("bank_ids", value)
                | ("preload_chr_sound_banks", value) => {
                    config.sound_bank_ids = parse_id_list(value);
                    sound_bank_ids_configured = true;
                }
                ("sound_bank_id", value)
                | ("bank_id", value)
                | ("preload_chr_sound_bank", value) => {
                    if !sound_bank_ids_configured {
                        config.sound_bank_ids.clear();
                        sound_bank_ids_configured = true;
                    }
                    config.sound_bank_ids.extend(parse_id_list(value));
                }
                ("start_delay_ms", value) => {
                    if let Ok(parsed) = value.parse::<u64>() {
                        config.start_delay_ms = parsed;
                    }
                }
                ("retries_per_id", value) => {
                    if let Ok(parsed) = value.parse::<u32>() {
                        config.retries_per_id = parsed.max(1);
                    }
                }
                ("retry_every_frames", value) => {
                    if let Ok(parsed) = value.parse::<u32>() {
                        config.retry_every_frames = parsed.max(1);
                    }
                }
                ("memory_copy_enabled", value)
                | ("load_bank_memory_copy", value)
                | ("wwise_memory_copy", value) => {
                    if let Some(parsed) = parse_bool(value) {
                        config.memory_copy_enabled = parsed;
                    }
                }
                ("bank_file_dirs", value) | ("bank_dirs", value) | ("sd_dirs", value) => {
                    config.bank_file_dirs = parse_path_list(value);
                    bank_file_dirs_configured = true;
                }
                ("bank_file_dir", value) | ("bank_dir", value) | ("sd_dir", value) => {
                    if !bank_file_dirs_configured {
                        config.bank_file_dirs.clear();
                        bank_file_dirs_configured = true;
                    }
                    config.bank_file_dirs.extend(parse_path_list(value));
                }
                ("conditional_enabled", value) | ("dynamic_enabled", value) => {
                    if let Some(parsed) = parse_bool(value) {
                        config.conditional_enabled = parsed;
                    }
                }
                ("condition_hand", value) | ("hand", value) => {
                    if let Some(parsed) = parse_hand_mode(value) {
                        config.condition_hand = parsed;
                    }
                }
                ("condition_bank_mappings", value)
                | ("sp_atkcategory_bank_mappings", value)
                | ("dynamic_bank_mappings", value) => {
                    config.condition_bank_mappings = parse_condition_bank_mappings(value);
                    condition_mappings_configured = true;
                }
                ("condition_bank_mapping", value)
                | ("sp_atkcategory_bank_mapping", value)
                | ("dynamic_bank_mapping", value) => {
                    if !condition_mappings_configured {
                        config.condition_bank_mappings.clear();
                        condition_mappings_configured = true;
                    }
                    config
                        .condition_bank_mappings
                        .extend(parse_condition_bank_mappings(value));
                }
                ("sp_effect_bank_mappings", value)
                | ("speffect_bank_mappings", value)
                | ("sp_effect_mappings", value) => {
                    config.sp_effect_bank_mappings = parse_condition_bank_mappings(value);
                    sp_effect_mappings_configured = true;
                }
                ("sp_effect_bank_mapping", value)
                | ("speffect_bank_mapping", value)
                | ("sp_effect_mapping", value) => {
                    if !sp_effect_mappings_configured {
                        config.sp_effect_bank_mappings.clear();
                        sp_effect_mappings_configured = true;
                    }
                    config
                        .sp_effect_bank_mappings
                        .extend(parse_condition_bank_mappings(value));
                }
                ("poll_every_frames", value) => {
                    if let Ok(parsed) = value.parse::<u32>() {
                        config.poll_every_frames = parsed.max(1);
                    }
                }
                ("unload_when_inactive", value) | ("dynamic_unload_enabled", value) => {
                    if let Some(parsed) = parse_bool(value) {
                        config.unload_when_inactive = parsed;
                    }
                }
                (key, value) => config.sfx.parse_entry(key, value),
            }
        }

        config.sound_bank_ids.sort_unstable();
        config.sound_bank_ids.dedup();
        config.bank_file_dirs = resolve_bank_file_dirs(hmodule, &config.bank_file_dirs);
        config
    }
}

fn selection(hand: HandMode) -> WeaponSelectionMode {
    match hand {
        HandMode::Left => WeaponSelectionMode::Even,
        HandMode::Right => WeaponSelectionMode::Odd,
        HandMode::Auto => WeaponSelectionMode::AutoTwoHand,
    }
}
fn condition_config(
    hand: HandMode,
    weapon: &[ConditionBankMapping],
    effects: &[ConditionBankMapping],
) -> SoundConditionReadConfig {
    let convert = |values: &[ConditionBankMapping]| {
        values
            .iter()
            .map(|v| SdkMapping {
                condition_id: v.condition_id,
                bank_id: v.bank_id,
            })
            .collect()
    };
    SoundConditionReadConfig {
        selection: selection(hand),
        weapon_mappings: convert(weapon),
        effect_mappings: convert(effects),
        ..Default::default()
    }
}
impl Config {
    fn schedule(&self) -> SoundBankScheduleConfig {
        SoundBankScheduleConfig {
            enabled: self.enabled,
            static_ids: if self.conditional_enabled {
                Vec::new()
            } else {
                self.sound_bank_ids.clone()
            },
            start_delay_ms: 0,
            attempts_per_id: self.retries_per_id,
            retry_every_frames: self.retry_every_frames,
            memory_copy_enabled: self.memory_copy_enabled,
            conditional_enabled: self.conditional_enabled,
            poll_every_frames: self.poll_every_frames,
            unload_when_inactive: self.unload_when_inactive,
        }
    }
}
struct Files {
    dirs: Vec<PathBuf>,
    log: Option<PathBuf>,
}
impl BankAssetSource for Files {
    fn read_bank(&mut self, id: u32) -> shikifsrs_runtime::Result<Vec<u8>> {
        let path = find_bank_file(id, &self.dirs).ok_or(
            shikifsrs_runtime::Error::NativeOperationRejected("bank file not found"),
        )?;
        let bytes = fs::read(&path).map_err(|error| {
            append_log(
                &self.log,
                &format!("bank id={id} read {} failed: {error}", path.display()),
            );
            shikifsrs_runtime::Error::NativeOperationRejected("bank file read failed")
        })?;
        append_log(
            &self.log,
            &format!(
                "bank id={id} source={} bytes={}",
                path.display(),
                bytes.len()
            ),
        );
        Ok(bytes)
    }
}
fn start_loader_after_delay(log: Option<PathBuf>, config: Config) {
    if let Err(error) = unsafe { install_current(&log, &config) } {
        append_log(
            &log,
            &format!("SDK sound installation failed: {error}; no automatic retry"),
        );
    }
}
/// # Safety
/// Loader retains this DLL until PIN. Caller supplies serialized registration/allocator and
/// complete initialized sound/resource graph, correct callback phases and coherent observations.
/// Initial epoch1 and original native SFX backend are experimental; no teardown/reuse may race.
unsafe fn install_current(log: &Option<PathBuf>, config: &Config) -> Result<(), String> {
    let bytes: Arc<[u8]> = fs::read(std::env::current_exe().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?
        .into();
    let file = Ww171File::verify(&bytes).map_err(|e| e.to_string())?;
    std::thread::sleep(Duration::from_millis(config.start_delay_ms));
    let base = unsafe { GetModuleHandleA(None) }
        .map_err(|e| e.to_string())?
        .0 as u64;
    let mut phases = vec![TaskPhase::SOUND_STEP, TaskPhase::FRAME_BEGIN];
    if config.sfx.enabled {
        phases.push(
            file.phase("FileStepUpdate_Begin")
                .map_err(|e| e.to_string())?,
        );
    }
    let started = Instant::now();
    loop {
        let catalog = file
            .bind(&CurrentProcess, base)
            .map_err(|e| e.to_string())?;
        let mut ready = true;
        for phase in &phases {
            match catalog
                .observe_phase(*phase)
                .map_err(|e| e.to_string())?
                .and_then(|v| v.native_group_id())
            {
                Some(id)
                    if catalog
                        .registered_group(id, 4096)
                        .map_err(|e| e.to_string())?
                        .is_some() => {}
                _ => {
                    ready = false;
                    break;
                }
            }
        }
        if ready {
            break;
        }
        if started.elapsed() >= Duration::from_secs(60) {
            return Err("current task groups unavailable after60seconds".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if config.sfx.enabled
        && let Err(error) = sfx::start(bytes.clone(), base, log.clone(), config)
    {
        append_log(
            log,
            &format!("SFX installation failed: {error}; independent sound path continues"),
        );
    }
    // Original conditional mode bypasses static preload. Empty/invalid conditions do not load static IDs.
    if config.conditional_enabled
        && (config.condition_bank_mappings.is_empty() && config.sp_effect_bank_mappings.is_empty()
            || !config.memory_copy_enabled)
    {
        append_log(
            log,
            "conditional mode has no mappings or memory_copy_enabled=false; SFX remains independent",
        );
        return Ok(());
    }
    if !config.conditional_enabled && config.sound_bank_ids.is_empty() {
        return Ok(());
    }
    let configured = ResidentSoundBankConfig {
        epoch: 1,
        schedule: config.schedule(),
        conditions: condition_config(
            config.condition_hand,
            &config.condition_bank_mappings,
            &config.sp_effect_bank_mappings,
        ),
        reference_limit: 1024,
    };
    let installed = unsafe {
        SOUND.install_current(
            bytes,
            base,
            configured,
            Files {
                dirs: config.bank_file_dirs.clone(),
                log: log.clone(),
            },
            None,
        )
    }
    .map_err(|e| e.to_string())?;
    let control = installed.registration.map_err(|e| e.to_string())?;
    append_log(
        log,
        &format!(
            "SDK process PIN={} registered_all={} startup={:?} conditional={:?}",
            installed.module_handle,
            control.registered_all(),
            control.startup_status(),
            control.conditional_status()
        ),
    );
    if !control.registered_all() {
        return Err(
            "partial sound installation retained; close queued on original callback".into(),
        );
    }
    let mut previous = String::new();
    loop {
        if let Some(report) = control.last_report() {
            let text = format!(
                "SDK sound state={:?} error={:?} schedule={:?} close={:?}",
                report.state, report.error, report.schedule, report.close
            );
            if text != previous {
                append_log(log, &text);
                previous = text;
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}
#[cfg(test)]
fn target_banks_for_conditions(
    sp_atkcategories: &HashSet<i32>,
    sp_atkcategory_mappings: &[ConditionBankMapping],
    sp_effect_ids: &HashSet<i32>,
    sp_effect_mappings: &[ConditionBankMapping],
) -> HashSet<u32> {
    let mut target_banks = sp_atkcategory_mappings
        .iter()
        .filter(|mapping| sp_atkcategories.contains(&mapping.condition_id))
        .map(|mapping| mapping.bank_id)
        .collect::<HashSet<_>>();
    target_banks.extend(
        sp_effect_mappings
            .iter()
            .filter(|mapping| sp_effect_ids.contains(&mapping.condition_id))
            .map(|mapping| mapping.bank_id),
    );
    target_banks
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    #[test]
    fn target_banks_are_union_of_all_player_conditions() {
        let condition_ids = HashSet::from([73, 122]);
        let mappings = [
            ConditionBankMapping {
                condition_id: 73,
                bank_id: 7800,
            },
            ConditionBankMapping {
                condition_id: 122,
                bank_id: 4205,
            },
            ConditionBankMapping {
                condition_id: 132,
                bank_id: 9495,
            },
        ];

        assert_eq!(
            target_banks_for_conditions(&condition_ids, &mappings, &HashSet::new(), &[],),
            HashSet::from([4205, 7800])
        );
    }

    #[test]
    fn shared_bank_is_deduplicated_across_conditions() {
        let condition_ids = HashSet::from([132, 133]);
        let mappings = [
            ConditionBankMapping {
                condition_id: 132,
                bank_id: 9495,
            },
            ConditionBankMapping {
                condition_id: 133,
                bank_id: 9495,
            },
        ];

        assert_eq!(
            target_banks_for_conditions(&condition_ids, &mappings, &HashSet::new(), &[],),
            HashSet::from([9495])
        );
    }

    #[test]
    fn weapon_and_sp_effect_triggers_form_bank_union() {
        let sp_atkcategories = HashSet::from([73]);
        let sp_effect_ids = HashSet::from([100_008]);
        let sp_atkcategory_mappings = [ConditionBankMapping {
            condition_id: 73,
            bank_id: 7800,
        }];
        let sp_effect_mappings = [ConditionBankMapping {
            condition_id: 100_008,
            bank_id: 4205,
        }];

        assert_eq!(
            target_banks_for_conditions(
                &sp_atkcategories,
                &sp_atkcategory_mappings,
                &sp_effect_ids,
                &sp_effect_mappings,
            ),
            HashSet::from([4205, 7800])
        );
    }

    #[test]
    fn shared_bank_is_deduplicated_across_trigger_types() {
        let sp_atkcategories = HashSet::from([132]);
        let sp_effect_ids = HashSet::from([100_008]);
        let sp_atkcategory_mappings = [ConditionBankMapping {
            condition_id: 132,
            bank_id: 9495,
        }];
        let sp_effect_mappings = [ConditionBankMapping {
            condition_id: 100_008,
            bank_id: 9495,
        }];

        assert_eq!(
            target_banks_for_conditions(
                &sp_atkcategories,
                &sp_atkcategory_mappings,
                &sp_effect_ids,
                &sp_effect_mappings,
            ),
            HashSet::from([9495])
        );
    }

    #[test]
    fn successful_load_is_owned_by_loader() {
        assert_eq!(
            bank_ownership_for_load_result(AK_SUCCESS),
            Some(BankOwnership::Loader)
        );
    }

    #[test]
    fn already_loaded_bank_is_externally_owned() {
        assert_eq!(
            bank_ownership_for_load_result(AK_BANK_ALREADY_LOADED),
            Some(BankOwnership::External)
        );
    }

    #[test]
    fn failed_load_has_no_ownership() {
        assert_eq!(bank_ownership_for_load_result(2), None);
    }

    #[test]
    fn fixed_sdk_gate_rejects_original_test_executable_without_loaded_reads() {
        let bytes = fs::read(std::env::current_exe().unwrap()).unwrap();
        assert!(Ww171File::verify(&bytes).is_err());
    }
}

fn find_bank_file(id: u32, bank_file_dirs: &[PathBuf]) -> Option<PathBuf> {
    let file_name = format!("cs_c{id:04}.bnk");
    bank_file_dirs
        .iter()
        .map(|dir| dir.join(&file_name))
        .find(|path| path.is_file())
}

fn parse_id_list(value: &str) -> Vec<u32> {
    value
        .split(|ch: char| ch == ',' || ch.is_ascii_whitespace())
        .filter_map(|part| {
            let trimmed = part.trim();
            if trimmed.is_empty() {
                None
            } else {
                trimmed.parse::<u32>().ok()
            }
        })
        .collect()
}

fn parse_path_list(value: &str) -> Vec<PathBuf> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn resolve_bank_file_dirs(hmodule: usize, configured: &[PathBuf]) -> Vec<PathBuf> {
    let auto_requested = configured.is_empty()
        || configured
            .iter()
            .any(|path| path.to_string_lossy().eq_ignore_ascii_case("auto"));

    let mut dirs = Vec::new();
    if auto_requested {
        add_auto_bank_file_dirs(&mut dirs, hmodule);
    }

    for path in configured {
        if path.to_string_lossy().eq_ignore_ascii_case("auto") {
            continue;
        }

        if path.is_absolute() {
            push_existing_unique_dir(&mut dirs, path.to_path_buf());
            continue;
        }

        for base in bank_dir_base_candidates(hmodule) {
            push_existing_unique_dir(&mut dirs, base.join(path));
        }
    }

    dirs
}

fn add_auto_bank_file_dirs(dirs: &mut Vec<PathBuf>, hmodule: usize) {
    for base in bank_dir_base_candidates(hmodule) {
        push_existing_unique_dir(dirs, base.join("sd"));
        push_existing_unique_dir(dirs, base.join("mod").join("sd"));

        for child in immediate_child_dirs(&base).into_iter().take(128) {
            push_existing_unique_dir(dirs, child.join("sd"));
            push_existing_unique_dir(dirs, child.join("mod").join("sd"));
        }
    }
}

fn bank_dir_base_candidates(hmodule: usize) -> Vec<PathBuf> {
    let mut bases = Vec::new();

    if let Some(module_dir) =
        module_path(hmodule).and_then(|path| path.parent().map(Path::to_path_buf))
    {
        push_unique_path(&mut bases, module_dir.clone());
        for ancestor in module_dir.ancestors() {
            push_unique_path(&mut bases, ancestor.to_path_buf());
        }
    }

    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        push_unique_path(&mut bases, exe_dir.to_path_buf());
        for ancestor in exe_dir.ancestors() {
            push_unique_path(&mut bases, ancestor.to_path_buf());
        }
    }

    if let Ok(current_dir) = std::env::current_dir() {
        push_unique_path(&mut bases, current_dir);
    }

    bases
}

fn immediate_child_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };

    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let Ok(file_type) = entry.file_type() else {
                return None;
            };
            file_type.is_dir().then(|| entry.path())
        })
        .collect()
}

fn push_existing_unique_dir(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if path.is_dir() {
        push_unique_path(paths, path);
    }
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| {
        existing
            .to_string_lossy()
            .eq_ignore_ascii_case(&path.to_string_lossy())
    }) {
        paths.push(path);
    }
}

fn parse_condition_bank_mappings(value: &str) -> Vec<ConditionBankMapping> {
    value
        .split(',')
        .filter_map(|entry| {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                return None;
            }

            let (condition, bank) = trimmed
                .split_once(':')
                .or_else(|| trimmed.split_once('='))
                .or_else(|| trimmed.split_once("->"))?;
            Some(ConditionBankMapping {
                condition_id: condition.trim().parse::<i32>().ok()?,
                bank_id: bank.trim().parse::<u32>().ok()?,
            })
        })
        .collect()
}

fn parse_hand_mode(value: &str) -> Option<HandMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" => Some(HandMode::Auto),
        "right" => Some(HandMode::Right),
        "left" => Some(HandMode::Left),
        _ => None,
    }
}

fn format_condition_bank_mappings(mappings: &[ConditionBankMapping]) -> String {
    mappings
        .iter()
        .map(|mapping| format!("{}:{}", mapping.condition_id, mapping.bank_id))
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
fn bank_ownership_for_load_result(result: i32) -> Option<BankOwnership> {
    match result {
        AK_SUCCESS => Some(BankOwnership::Loader),
        AK_BANK_ALREADY_LOADED => Some(BankOwnership::External),
        _ => None,
    }
}

fn sidecar_path(hmodule: usize, file_name: &str) -> Option<PathBuf> {
    let mut path = module_path(hmodule)?;
    path.set_file_name(file_name);
    Some(path)
}

fn module_path(hmodule: usize) -> Option<PathBuf> {
    let mut buffer = [0u16; 260];
    let len =
        unsafe { GetModuleFileNameW(Some(HMODULE(hmodule as *mut c_void)), &mut buffer) } as usize;
    if len == 0 {
        return None;
    }

    Some(PathBuf::from(String::from_utf16_lossy(&buffer[..len])))
}

fn append_log(path: &Option<PathBuf>, message: &str) {
    let Some(path) = path else {
        return;
    };

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{message}");
    }
}

#[cfg(test)]
#[path = "sdk_tests.rs"]
mod sdk_tests;
