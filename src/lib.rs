use std::{
    collections::{HashMap, HashSet},
    ffi::c_void,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

use eldenring::{
    cs::{
        CSTaskGroupIndex, CSTaskImp, ChrAsm, ChrAsmArmStyle, EquipParamWeapon, SoloParamRepository,
        WorldChrMan,
    },
    fd4::FD4TaskData,
};
use fromsoftware_shared::{FromStatic, SharedTaskImpExt};
use windows::Win32::{
    Foundation::HMODULE,
    System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleA},
};

const FILECAP_REQUEST_FN_RVA: usize = 0x001F2420;
const LOAD_BANK_MEMORY_COPY_FN_RVA: usize = 0x02239630;
const UNLOAD_BANK_FN_RVA: usize = 0x022404E0;
const AK_SUCCESS: i32 = 1;
const AK_BANK_ALREADY_LOADED: i32 = 69;

#[repr(C)]
#[fromsoftware_shared::singleton("CSFD4MoWwisebankRepository")]
struct CSFD4MoWwisebankRepository {
    _opaque: [u8; 0],
}

#[derive(Clone)]
struct Config {
    enabled: bool,
    log_enabled: bool,
    sound_bank_ids: Vec<u32>,
    start_delay_ms: u64,
    retries_per_id: u32,
    retry_every_frames: u32,
    path_templates: Vec<String>,
    memory_copy_enabled: bool,
    bank_file_dirs: Vec<PathBuf>,
    conditional_enabled: bool,
    condition_hand: HandMode,
    condition_bank_mappings: Vec<ConditionBankMapping>,
    poll_every_frames: u32,
    unload_when_inactive: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            log_enabled: true,
            sound_bank_ids: vec![4205, 7800, 8019, 9495, 9496],
            start_delay_ms: 5000,
            retries_per_id: 120,
            retry_every_frames: 60,
            path_templates: vec![
                "wwise:/{id}.bnk".to_string(),
                "wwise:/cs_c{id4}.bnk".to_string(),
                "wwise_bnd:/cs_c{id4}.bnk".to_string(),
            ],
            memory_copy_enabled: true,
            bank_file_dirs: vec![PathBuf::from("auto")],
            conditional_enabled: true,
            condition_hand: HandMode::Auto,
            condition_bank_mappings: vec![
                ConditionBankMapping {
                    condition_id: 73,
                    bank_id: 7800,
                },
                ConditionBankMapping {
                    condition_id: 105,
                    bank_id: 8019,
                },
                ConditionBankMapping {
                    condition_id: 122,
                    bank_id: 4205,
                },
                ConditionBankMapping {
                    condition_id: 132,
                    bank_id: 9495,
                },
                ConditionBankMapping {
                    condition_id: 133,
                    bank_id: 9495,
                },
                ConditionBankMapping {
                    condition_id: 643,
                    bank_id: 9496,
                },
            ],
            poll_every_frames: 6,
            unload_when_inactive: true,
        }
    }
}

#[derive(Clone)]
struct PendingBank {
    id: u32,
    attempts: u32,
    done: bool,
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

#[derive(Clone, Copy)]
enum ActiveHand {
    Left,
    Right,
}

#[derive(Default)]
struct ConditionalState {
    frame: u32,
    last_condition_id: Option<i32>,
    loaded_banks: HashMap<u32, u32>,
}

struct LoadBankResult {
    loaded_bank_id: u32,
    message: String,
}

type FilecapRequestFn = unsafe extern "system" fn(*mut c_void, *const u16, usize) -> *mut c_void;
type LoadBankMemoryCopyFn = unsafe extern "system" fn(*const c_void, u32, *mut u32) -> i32;
type UnloadBankFn = unsafe extern "system" fn(u32, *const c_void) -> i32;

/// # Safety
///
/// This is the Windows DLL entry point. It is called by the loader.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn DllMain(hmodule: usize, reason: u32, _reserved: usize) -> bool {
    if reason != 1 {
        return true;
    }

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
                "loaded enabled={} log_enabled={} sound_bank_ids={:?} start_delay_ms={} retries_per_id={} retry_every_frames={} path_templates={:?} memory_copy_enabled={} bank_file_dirs={:?} conditional_enabled={} condition_hand={:?} condition_bank_mappings={} poll_every_frames={} unload_when_inactive={}",
                config.enabled,
                config.log_enabled,
                config.sound_bank_ids,
                config.start_delay_ms,
                config.retries_per_id,
                config.retry_every_frames,
                config.path_templates,
                config.memory_copy_enabled,
                config.bank_file_dirs,
                config.conditional_enabled,
                config.condition_hand,
                format_condition_bank_mappings(&config.condition_bank_mappings),
                config.poll_every_frames,
                config.unload_when_inactive,
            ),
        );

        if !config.enabled {
            append_log(&log_path, "loader disabled by config");
            return;
        }

        if !config.conditional_enabled && config.sound_bank_ids.is_empty() {
            append_log(&log_path, "no sound_bank_ids configured");
            return;
        }

        if config.path_templates.is_empty() {
            append_log(&log_path, "no path_templates configured");
            return;
        }

        start_loader_after_delay(log_path, config);
    });

    true
}

impl Config {
    fn load(hmodule: usize) -> Self {
        let mut config = Self::default();
        let mut sound_bank_ids_configured = false;
        let mut path_templates_configured = false;
        let mut bank_file_dirs_configured = false;
        let mut condition_mappings_configured = false;

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
                ("path_templates", value) | ("candidate_paths", value) => {
                    config.path_templates = parse_string_list(value);
                    path_templates_configured = true;
                }
                ("path_template", value) | ("candidate_path", value) => {
                    if !path_templates_configured {
                        config.path_templates.clear();
                        path_templates_configured = true;
                    }
                    config.path_templates.extend(parse_string_list(value));
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
                _ => {}
            }
        }

        config.sound_bank_ids.sort_unstable();
        config.sound_bank_ids.dedup();
        config.bank_file_dirs = resolve_bank_file_dirs(hmodule, &config.bank_file_dirs);
        config
    }
}

fn start_loader_after_delay(log_path: Option<PathBuf>, config: Config) {
    std::thread::sleep(Duration::from_millis(config.start_delay_ms));

    let Ok(cs_task) = CSTaskImp::wait_for_instance(Duration::MAX) else {
        append_log(&log_path, "failed: CSTaskImp instance not found");
        return;
    };

    if config.conditional_enabled {
        start_conditional_scheduler(cs_task, log_path, config);
        return;
    }

    append_log(
        &log_path,
        "startup preload scheduler installed on SoundStep",
    );

    let mut frame = 0u32;
    let mut pending = config
        .sound_bank_ids
        .iter()
        .copied()
        .map(|id| PendingBank {
            id,
            attempts: 0,
            done: false,
        })
        .collect::<Vec<_>>();
    let retry_every_frames = config.retry_every_frames;
    let retries_per_id = config.retries_per_id;
    let path_templates = config.path_templates.clone();
    let memory_copy_enabled = config.memory_copy_enabled;
    let bank_file_dirs = config.bank_file_dirs.clone();
    let mut finished = false;

    cs_task.run_recurring(
        move |_: &FD4TaskData| {
            if finished {
                return;
            }

            frame = frame.wrapping_add(1);
            if frame % retry_every_frames != 1 {
                return;
            }

            let mut all_done = true;
            for pending_bank in &mut pending {
                if pending_bank.done {
                    continue;
                }

                if pending_bank.attempts >= retries_per_id {
                    append_log(
                        &log_path,
                        &format!(
                            "bank id={} stopped after {} attempts",
                            pending_bank.id, retries_per_id
                        ),
                    );
                    pending_bank.done = true;
                    continue;
                }

                pending_bank.attempts = pending_bank.attempts.saturating_add(1);
                match unsafe {
                    request_sound_bank(
                        pending_bank.id,
                        &path_templates,
                        memory_copy_enabled,
                        &bank_file_dirs,
                    )
                } {
                    Ok(message) => {
                        append_log(
                            &log_path,
                            &format!(
                                "bank id={} attempt={} {message}",
                                pending_bank.id, pending_bank.attempts
                            ),
                        );
                        pending_bank.done = true;
                    }
                    Err(message) => {
                        append_log(
                            &log_path,
                            &format!(
                                "bank id={} attempt={} failed: {message}",
                                pending_bank.id, pending_bank.attempts
                            ),
                        );
                        all_done = false;
                    }
                }
            }

            if all_done || pending.iter().all(|item| item.done) {
                append_log(&log_path, "sound_bank_loader finished");
                finished = true;
            }
        },
        CSTaskGroupIndex::SoundStep,
    );
}

fn start_conditional_scheduler(
    cs_task: &'static CSTaskImp,
    log_path: Option<PathBuf>,
    config: Config,
) {
    if config.condition_bank_mappings.is_empty() {
        append_log(&log_path, "conditional mode has no condition_bank_mappings");
        return;
    }
    if !config.memory_copy_enabled {
        append_log(
            &log_path,
            "conditional mode requires memory_copy_enabled=true",
        );
        return;
    }

    append_log(
        &log_path,
        "conditional sound_bank_loader scheduler installed on FrameBegin",
    );

    let path_templates = config.path_templates.clone();
    let bank_file_dirs = config.bank_file_dirs.clone();
    let mappings = config.condition_bank_mappings.clone();
    let hand = config.condition_hand;
    let poll_every_frames = config.poll_every_frames.max(1);
    let unload_when_inactive = config.unload_when_inactive;
    let mut state = ConditionalState::default();

    cs_task.run_recurring(
        move |_: &FD4TaskData| {
            state.frame = state.frame.wrapping_add(1);
            if poll_every_frames > 1 && !state.frame.is_multiple_of(poll_every_frames) {
                return;
            }

            let condition_id = detect_sp_atkcategory(hand);
            if state.last_condition_id != condition_id {
                append_log(
                    &log_path,
                    &format!("condition changed sp_atkcategory={condition_id:?}"),
                );
                state.last_condition_id = condition_id;
            }

            let target_banks = target_banks_for_condition(condition_id, &mappings);
            if unload_when_inactive {
                unload_inactive_banks(&log_path, &target_banks, &mut state);
            }
            load_missing_banks(
                &log_path,
                &target_banks,
                &path_templates,
                &bank_file_dirs,
                &mut state,
            );
        },
        CSTaskGroupIndex::FrameBegin,
    );
}

fn target_banks_for_condition(
    condition_id: Option<i32>,
    mappings: &[ConditionBankMapping],
) -> HashSet<u32> {
    let Some(condition_id) = condition_id else {
        return HashSet::new();
    };

    mappings
        .iter()
        .filter(|mapping| mapping.condition_id == condition_id)
        .map(|mapping| mapping.bank_id)
        .collect()
}

fn load_missing_banks(
    log_path: &Option<PathBuf>,
    target_banks: &HashSet<u32>,
    path_templates: &[String],
    bank_file_dirs: &[PathBuf],
    state: &mut ConditionalState,
) {
    let Ok(exe) = (unsafe { GetModuleHandleA(None) }) else {
        append_log(
            log_path,
            "conditional load failed: GetModuleHandleA(None) failed",
        );
        return;
    };
    let base = exe.0 as usize;

    for bank_id in target_banks {
        if state.loaded_banks.contains_key(bank_id) {
            continue;
        }

        let filecap_message = unsafe { request_sound_bank_filecap(base, *bank_id, path_templates) }
            .unwrap_or_else(|message| format!("filecap failed: {message}"));
        match unsafe { load_bank_memory_copy_result(base, *bank_id, bank_file_dirs) } {
            Ok(result) => {
                state.loaded_banks.insert(*bank_id, result.loaded_bank_id);
                append_log(
                    log_path,
                    &format!(
                        "conditional loaded bank id={bank_id}; {filecap_message}; {}",
                        result.message
                    ),
                );
            }
            Err(message) => {
                append_log(
                    log_path,
                    &format!(
                        "conditional load failed bank id={bank_id}; {filecap_message}; {message}"
                    ),
                );
            }
        }
    }
}

fn unload_inactive_banks(
    log_path: &Option<PathBuf>,
    target_banks: &HashSet<u32>,
    state: &mut ConditionalState,
) {
    let inactive = state
        .loaded_banks
        .keys()
        .copied()
        .filter(|bank_id| !target_banks.contains(bank_id))
        .collect::<Vec<_>>();
    if inactive.is_empty() {
        return;
    }

    let Ok(exe) = (unsafe { GetModuleHandleA(None) }) else {
        append_log(
            log_path,
            "conditional unload failed: GetModuleHandleA(None) failed",
        );
        return;
    };
    let base = exe.0 as usize;

    for bank_id in inactive {
        let Some(loaded_bank_id) = state.loaded_banks.remove(&bank_id) else {
            continue;
        };
        let message = unsafe { unload_bank(base, loaded_bank_id) };
        append_log(
            log_path,
            &format!(
                "conditional unloaded bank id={bank_id} loaded_bank_id=0x{loaded_bank_id:08X}; {message}"
            ),
        );
    }
}

unsafe fn request_sound_bank(
    id: u32,
    path_templates: &[String],
    memory_copy_enabled: bool,
    bank_file_dirs: &[PathBuf],
) -> Result<String, String> {
    let exe = unsafe { GetModuleHandleA(None) }
        .map_err(|err| format!("GetModuleHandleA(None) failed: {err:?}"))?;
    let base = exe.0 as usize;

    let filecap_message = unsafe { request_sound_bank_filecap(base, id, path_templates) };

    if !memory_copy_enabled {
        return filecap_message;
    }

    let memory_message = unsafe {
        load_bank_memory_copy_result(base, id, bank_file_dirs).map(|result| result.message)
    };
    match (filecap_message, memory_message) {
        (Ok(filecap), Ok(memory)) => Ok(format!("{filecap}; {memory}")),
        (Ok(filecap), Err(memory)) => Err(format!("{filecap}; memory_copy failed: {memory}")),
        (Err(filecap), Ok(memory)) => Ok(format!("filecap failed: {filecap}; {memory}")),
        (Err(filecap), Err(memory)) => Err(format!(
            "filecap failed: {filecap}; memory_copy failed: {memory}"
        )),
    }
}

unsafe fn request_sound_bank_filecap(
    base: usize,
    id: u32,
    path_templates: &[String],
) -> Result<String, String> {
    let repository = CSFD4MoWwisebankRepository::instance_ptr()
        .map_err(|err| format!("CSFD4MoWwisebankRepository instance not found: {err}"))?
        as *mut c_void;
    if repository.is_null() {
        return Err("CSFD4MoWwisebankRepository instance is null".to_string());
    }

    let request_addr = base + FILECAP_REQUEST_FN_RVA;
    let request: FilecapRequestFn = unsafe { std::mem::transmute(request_addr) };
    let mut failures = Vec::new();

    for template in path_templates {
        let path = expand_path_template(template, id);
        let mut wide_path = path.encode_utf16().collect::<Vec<_>>();
        wide_path.push(0);

        let filecap = unsafe { request(repository, wide_path.as_ptr(), 0) };
        if !filecap.is_null() {
            return Ok(format!(
                "filecap_request path={path} fn=eldenring.exe+0x{FILECAP_REQUEST_FN_RVA:X} addr=0x{request_addr:X} repository=0x{:X} result=0x{:X}",
                repository as usize, filecap as usize
            ));
        }

        failures.push(format!("{path}=0x0"));
    }

    Err(format!(
        "all candidates returned null fn=eldenring.exe+0x{FILECAP_REQUEST_FN_RVA:X} addr=0x{request_addr:X} repository=0x{:X} candidates=[{}]",
        repository as usize,
        failures.join(", ")
    ))
}

unsafe fn load_bank_memory_copy_result(
    base: usize,
    id: u32,
    bank_file_dirs: &[PathBuf],
) -> Result<LoadBankResult, String> {
    let Some(bank_path) = find_bank_file(id, bank_file_dirs) else {
        return Err(format!(
            "bank file not found for id={id} searched_dirs={bank_file_dirs:?}"
        ));
    };

    let bytes = fs::read(&bank_path)
        .map_err(|err| format!("read {} failed: {err}", bank_path.display()))?;
    if bytes.is_empty() {
        return Err(format!("{} is empty", bank_path.display()));
    }
    if bytes.len() > u32::MAX as usize {
        return Err(format!(
            "{} is too large: {}",
            bank_path.display(),
            bytes.len()
        ));
    }

    let fn_addr = base + LOAD_BANK_MEMORY_COPY_FN_RVA;
    let load_bank_memory_copy: LoadBankMemoryCopyFn = unsafe { std::mem::transmute(fn_addr) };
    let mut loaded_bank_id = 0u32;
    let result = unsafe {
        load_bank_memory_copy(
            bytes.as_ptr() as *const c_void,
            bytes.len() as u32,
            &mut loaded_bank_id,
        )
    };

    let result_name = ak_result_name(result);
    let message = format!(
        "memory_copy path={} size={} fn=eldenring.exe+0x{LOAD_BANK_MEMORY_COPY_FN_RVA:X} addr=0x{fn_addr:X} ak_result={}({result_name}) loaded_bank_id={} 0x{:08X}",
        bank_path.display(),
        bytes.len(),
        result,
        loaded_bank_id,
        loaded_bank_id
    );

    if is_ak_load_ok(result) {
        Ok(LoadBankResult {
            loaded_bank_id,
            message,
        })
    } else {
        Err(message)
    }
}

unsafe fn unload_bank(base: usize, loaded_bank_id: u32) -> String {
    let fn_addr = base + UNLOAD_BANK_FN_RVA;
    let unload_bank: UnloadBankFn = unsafe { std::mem::transmute(fn_addr) };
    let result = unsafe { unload_bank(loaded_bank_id, std::ptr::null()) };
    let result_name = ak_result_name(result);

    format!(
        "unload_bank fn=eldenring.exe+0x{UNLOAD_BANK_FN_RVA:X} addr=0x{fn_addr:X} ak_result={}({result_name})",
        result
    )
}

fn detect_sp_atkcategory(hand_mode: HandMode) -> Option<i32> {
    let world_chr_man = unsafe { WorldChrMan::instance() }.ok()?;
    let player = world_chr_man.main_player.as_deref()?;
    let chr_asm = player.chr_asm.as_ref();
    let hand = active_hand(hand_mode, chr_asm);
    let slot_index = active_weapon_slot_index(chr_asm, hand)?;
    let param_id = chr_asm.equipment_param_ids.get(slot_index).copied()?;
    if param_id <= 0 {
        return None;
    }

    let repo = unsafe { SoloParamRepository::instance() }.ok()?;
    let weapon = repo.get::<EquipParamWeapon>(((param_id as u32) / 100) * 100)?;
    Some(weapon.sp_atkcategory() as i32)
}

fn active_hand(hand_mode: HandMode, chr_asm: &ChrAsm) -> ActiveHand {
    match hand_mode {
        HandMode::Left => ActiveHand::Left,
        HandMode::Right => ActiveHand::Right,
        HandMode::Auto => match chr_asm.equipment.arm_style {
            ChrAsmArmStyle::LeftBothHands => ActiveHand::Left,
            _ => ActiveHand::Right,
        },
    }
}

fn active_weapon_slot_index(chr_asm: &ChrAsm, hand: ActiveHand) -> Option<usize> {
    let slots = &chr_asm.equipment.selected_slots;
    let slot = match hand {
        ActiveHand::Left => slots.left_weapon_slot,
        ActiveHand::Right => slots.right_weapon_slot,
    };

    if slot > 2 {
        return None;
    }

    Some(match hand {
        ActiveHand::Left => (slot * 2) as usize,
        ActiveHand::Right => (slot * 2 + 1) as usize,
    })
}

fn find_bank_file(id: u32, bank_file_dirs: &[PathBuf]) -> Option<PathBuf> {
    let file_name = format!("cs_c{id:04}.bnk");
    bank_file_dirs
        .iter()
        .map(|dir| dir.join(&file_name))
        .find(|path| path.is_file())
}

fn expand_path_template(template: &str, id: u32) -> String {
    template
        .replace("{id4}", &format!("{id:04}"))
        .replace("{id}", &id.to_string())
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

fn parse_string_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
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

fn is_ak_load_ok(result: i32) -> bool {
    result == AK_SUCCESS || result == AK_BANK_ALREADY_LOADED
}

fn ak_result_name(result: i32) -> &'static str {
    match result {
        1 => "AK_Success",
        2 => "AK_Fail",
        7 => "AK_InvalidFile",
        14 => "AK_InvalidID",
        16 => "AK_NoMoreData",
        31 => "AK_InvalidParameter",
        52 => "AK_InsufficientMemory",
        56 => "AK_UnknownBankID",
        64 => "AK_BankReadError",
        65 => "AK_InvalidSwitchType",
        66 => "AK_FormatNotReady",
        67 => "AK_WrongBankVersion",
        68 => "AK_FileNotFound",
        69 => "AK_DeviceNotReady_or_BankAlreadyLoaded",
        70 => "AK_BankAlreadyLoaded_or_RenderedFX",
        _ => "unknown",
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
