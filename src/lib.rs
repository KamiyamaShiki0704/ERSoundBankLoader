use std::{
    collections::{HashMap, HashSet},
    ffi::c_void,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use eldenring::{
    cs::{
        CSTaskGroupIndex, CSTaskImp, ChrAsm, ChrAsmArmStyle, EquipParamWeapon, SoloParamRepository,
        WorldChrMan,
    },
    fd4::FD4TaskData,
};
use fromsoftware_shared::{FromStatic, InstanceError, RecurringTask};
use pelite::pe64::{Pe, PeView};
use windows::Win32::{
    Foundation::HMODULE,
    System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleA},
};

const AK_SUCCESS: i32 = 1;
const AK_BANK_ALREADY_LOADED: i32 = 69;
const LOAD_BANK_MEMORY_COPY_EXPORT: &[u8] =
    b"?LoadBankMemoryCopy@SoundEngine@AK@@YA?AW4AKRESULT@@PEBXKAEAK@Z";
const UNLOAD_BANK_EXPORT: &[u8] = b"?UnloadBank@SoundEngine@AK@@YA?AW4AKRESULT@@KPEBX@Z";
const REGISTER_TASK_PATTERN: &[pelite::pattern::Atom] =
    pelite::pattern!("e8 ? ? ? ? 48 8b 0d ? ? ? ? 4c 8b c7 8b d3 e8 $ { ' }");

#[derive(Clone)]
struct Config {
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
    last_player_count: Option<usize>,
    last_sp_atkcategories: HashSet<i32>,
    last_sp_effect_ids: HashSet<i32>,
    loaded_banks: HashMap<u32, TrackedBank>,
}

#[derive(Default)]
struct ConditionSnapshot {
    player_count: usize,
    sp_atkcategories: HashSet<i32>,
    sp_effect_ids: HashSet<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BankOwnership {
    Loader,
    External,
}

impl BankOwnership {
    fn as_str(self) -> &'static str {
        match self {
            Self::Loader => "loader",
            Self::External => "external",
        }
    }
}

#[derive(Clone, Copy)]
struct TrackedBank {
    loaded_bank_id: u32,
    ownership: BankOwnership,
}

struct LoadBankResult {
    tracked_bank: TrackedBank,
    message: String,
}

type LoadBankMemoryCopyFn = unsafe extern "system" fn(*const c_void, u32, *mut u32) -> i32;
type UnloadBankFn = unsafe extern "system" fn(u32, *const c_void) -> i32;
type RegisterTaskFn =
    unsafe extern "C" fn(&CSTaskImp, CSTaskGroupIndex, &RecurringTask<FD4TaskData>);

#[derive(Clone, Copy)]
struct RuntimeApi {
    register_task: RegisterTaskFn,
    register_task_rva: u32,
    load_bank_memory_copy: LoadBankMemoryCopyFn,
    load_bank_memory_copy_rva: u32,
    unload_bank: UnloadBankFn,
    unload_bank_rva: u32,
}

impl RuntimeApi {
    unsafe fn resolve() -> Result<Self, String> {
        let exe = unsafe { GetModuleHandleA(None) }
            .map_err(|err| format!("GetModuleHandleA(None) failed: {err:?}"))?;
        let base = exe.0 as usize;
        if base == 0 {
            return Err("eldenring.exe module base is null".to_string());
        }

        let pe = unsafe { PeView::module(base as *const u8) };
        let register_task_rva = resolve_register_task_rva(pe)?;
        let load_bank_memory_copy_rva = resolve_export_rva(pe, LOAD_BANK_MEMORY_COPY_EXPORT)?;
        let unload_bank_rva = resolve_export_rva(pe, UNLOAD_BANK_EXPORT)?;

        Ok(Self {
            register_task: unsafe {
                std::mem::transmute::<usize, RegisterTaskFn>(base + register_task_rva as usize)
            },
            register_task_rva,
            load_bank_memory_copy: unsafe {
                std::mem::transmute::<usize, LoadBankMemoryCopyFn>(
                    base + load_bank_memory_copy_rva as usize,
                )
            },
            load_bank_memory_copy_rva,
            unload_bank: unsafe {
                std::mem::transmute::<usize, UnloadBankFn>(base + unload_bank_rva as usize)
            },
            unload_bank_rva,
        })
    }
}

fn resolve_export_rva<'a, P: Pe<'a>>(pe: P, name: &[u8]) -> Result<u32, String> {
    pe.exports()
        .and_then(|exports| exports.by())
        .and_then(|exports| exports.name(name))
        .and_then(|export| export.symbol().ok_or(pelite::Error::Null))
        .map_err(|err| {
            format!(
                "required export {} was not found: {err:?}",
                String::from_utf8_lossy(name)
            )
        })
}

fn resolve_register_task_rva<'a, P: Pe<'a>>(pe: P) -> Result<u32, String> {
    let mut captures = [0; 2];
    if !pe
        .scanner()
        .finds_code(REGISTER_TASK_PATTERN, &mut captures)
    {
        return Err("register_task signature was missing or ambiguous".to_string());
    }

    let rva = captures[1];
    if rva == 0 {
        return Err("register_task signature resolved a null RVA".to_string());
    }
    Ok(rva)
}

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
                "loaded enabled={} log_enabled={} sound_bank_ids={:?} start_delay_ms={} retries_per_id={} retry_every_frames={} memory_copy_enabled={} bank_file_dirs={:?} conditional_enabled={} condition_hand={:?} condition_bank_mappings={} sp_effect_bank_mappings={} poll_every_frames={} unload_when_inactive={}",
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

        start_loader_after_delay(log_path, config);
    });

    true
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

    let api = match unsafe { RuntimeApi::resolve() } {
        Ok(api) => api,
        Err(message) => {
            append_log(
                &log_path,
                &format!("failed to resolve runtime API: {message}"),
            );
            return;
        }
    };
    append_log(
        &log_path,
        &format!(
            "runtime API resolved register_task=eldenring.exe+0x{:X} LoadBankMemoryCopy=eldenring.exe+0x{:X} UnloadBank=eldenring.exe+0x{:X}",
            api.register_task_rva, api.load_bank_memory_copy_rva, api.unload_bank_rva
        ),
    );

    let cs_task = match wait_for_cs_task() {
        Ok(cs_task) => cs_task,
        Err(message) => {
            append_log(&log_path, &format!("failed: {message}"));
            return;
        }
    };

    if config.conditional_enabled {
        start_conditional_scheduler(cs_task, api, log_path, config);
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
    let memory_copy_enabled = config.memory_copy_enabled;
    let bank_file_dirs = config.bank_file_dirs.clone();
    let mut finished = false;

    install_recurring_task(
        cs_task,
        api,
        CSTaskGroupIndex::SoundStep,
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
                    request_sound_bank(pending_bank.id, memory_copy_enabled, &bank_file_dirs, api)
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
    );
}

fn start_conditional_scheduler(
    cs_task: &'static CSTaskImp,
    api: RuntimeApi,
    log_path: Option<PathBuf>,
    config: Config,
) {
    if config.condition_bank_mappings.is_empty() && config.sp_effect_bank_mappings.is_empty() {
        append_log(
            &log_path,
            "conditional mode has no condition_bank_mappings or sp_effect_bank_mappings",
        );
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
        "conditional sound_bank_loader scheduler installed on FrameBegin; scanning all players",
    );

    let bank_file_dirs = config.bank_file_dirs.clone();
    let sp_atkcategory_mappings = config.condition_bank_mappings.clone();
    let sp_effect_mappings = config.sp_effect_bank_mappings.clone();
    let watched_sp_effect_ids = sp_effect_mappings
        .iter()
        .map(|mapping| mapping.condition_id)
        .collect::<HashSet<_>>();
    let hand = config.condition_hand;
    let poll_every_frames = config.poll_every_frames.max(1);
    let unload_when_inactive = config.unload_when_inactive;
    let mut state = ConditionalState::default();

    install_recurring_task(
        cs_task,
        api,
        CSTaskGroupIndex::FrameBegin,
        move |_: &FD4TaskData| {
            state.frame = state.frame.wrapping_add(1);
            if poll_every_frames > 1 && !state.frame.is_multiple_of(poll_every_frames) {
                return;
            }

            let snapshot = detect_player_conditions(hand, &watched_sp_effect_ids);
            if state.last_player_count != Some(snapshot.player_count)
                || state.last_sp_atkcategories != snapshot.sp_atkcategories
                || state.last_sp_effect_ids != snapshot.sp_effect_ids
            {
                let mut sorted_sp_atkcategories = snapshot
                    .sp_atkcategories
                    .iter()
                    .copied()
                    .collect::<Vec<_>>();
                sorted_sp_atkcategories.sort_unstable();
                let mut sorted_sp_effect_ids =
                    snapshot.sp_effect_ids.iter().copied().collect::<Vec<_>>();
                sorted_sp_effect_ids.sort_unstable();
                append_log(
                    &log_path,
                    &format!(
                        "player conditions changed players={} sp_atkcategories={sorted_sp_atkcategories:?} sp_effects={sorted_sp_effect_ids:?}",
                        snapshot.player_count
                    ),
                );
                state.last_player_count = Some(snapshot.player_count);
                state.last_sp_atkcategories = snapshot.sp_atkcategories.clone();
                state.last_sp_effect_ids = snapshot.sp_effect_ids.clone();
            }

            let target_banks = target_banks_for_conditions(
                &snapshot.sp_atkcategories,
                &sp_atkcategory_mappings,
                &snapshot.sp_effect_ids,
                &sp_effect_mappings,
            );
            if unload_when_inactive {
                unload_inactive_banks(&log_path, &target_banks, &mut state, api);
            }
            load_missing_banks(&log_path, &target_banks, &bank_file_dirs, &mut state, api);
        },
    );
}

fn wait_for_cs_task() -> Result<&'static CSTaskImp, String> {
    let started = Instant::now();
    let timeout = Duration::from_secs(60);

    loop {
        let current_error = match unsafe { CSTaskImp::instance() } {
            Ok(cs_task) => return Ok(cs_task),
            Err(InstanceError::NotFound(name)) => format!("singleton not found: {name}"),
            Err(InstanceError::Null(name)) => format!("singleton not initialized: {name}"),
        };

        if started.elapsed() >= timeout {
            return Err(format!(
                "CSTaskImp was unavailable after {} seconds ({})",
                timeout.as_secs(),
                current_error
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn install_recurring_task<F>(
    cs_task: &'static CSTaskImp,
    api: RuntimeApi,
    group: CSTaskGroupIndex,
    execute: F,
) where
    F: FnMut(&FD4TaskData) + Send + 'static,
{
    // The game owns this task for the remainder of the DLL's process lifetime.
    let task = Box::leak(Box::new(RecurringTask::new(execute)));
    unsafe { (api.register_task)(cs_task, group, task) };
}

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

fn load_missing_banks(
    log_path: &Option<PathBuf>,
    target_banks: &HashSet<u32>,
    bank_file_dirs: &[PathBuf],
    state: &mut ConditionalState,
    api: RuntimeApi,
) {
    for bank_id in target_banks {
        if state.loaded_banks.contains_key(bank_id) {
            continue;
        }

        match unsafe { load_bank_memory_copy_result(api, *bank_id, bank_file_dirs) } {
            Ok(result) => {
                state.loaded_banks.insert(*bank_id, result.tracked_bank);
                append_log(
                    log_path,
                    &format!(
                        "conditional loaded bank id={bank_id} ownership={}; {}",
                        result.tracked_bank.ownership.as_str(),
                        result.message,
                    ),
                );
            }
            Err(message) => {
                append_log(
                    log_path,
                    &format!("conditional load failed bank id={bank_id}; {message}"),
                );
            }
        }
    }
}

fn unload_inactive_banks(
    log_path: &Option<PathBuf>,
    target_banks: &HashSet<u32>,
    state: &mut ConditionalState,
    api: RuntimeApi,
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

    for bank_id in inactive {
        let Some(tracked_bank) = state.loaded_banks.remove(&bank_id) else {
            continue;
        };

        if tracked_bank.ownership == BankOwnership::External {
            append_log(
                log_path,
                &format!(
                    "conditional released bank id={bank_id} loaded_bank_id=0x{:08X}; unload skipped ownership=external (bank pre-existed in Wwise)",
                    tracked_bank.loaded_bank_id
                ),
            );
            continue;
        }

        let message = unsafe { unload_bank(api, tracked_bank.loaded_bank_id) };
        append_log(
            log_path,
            &format!(
                "conditional unloaded bank id={bank_id} loaded_bank_id=0x{:08X} ownership=loader; {message}",
                tracked_bank.loaded_bank_id
            ),
        );
    }
}

unsafe fn request_sound_bank(
    id: u32,
    memory_copy_enabled: bool,
    bank_file_dirs: &[PathBuf],
    api: RuntimeApi,
) -> Result<String, String> {
    if !memory_copy_enabled {
        return Err(
            "memory_copy_enabled=false is unsupported; FD4 filecap loading was removed".to_string(),
        );
    }

    unsafe { load_bank_memory_copy_result(api, id, bank_file_dirs) }.map(|result| result.message)
}

unsafe fn load_bank_memory_copy_result(
    api: RuntimeApi,
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

    let mut loaded_bank_id = 0u32;
    let result = unsafe {
        (api.load_bank_memory_copy)(
            bytes.as_ptr() as *const c_void,
            bytes.len() as u32,
            &mut loaded_bank_id,
        )
    };

    let result_name = ak_result_name(result);
    let message = format!(
        "memory_copy path={} size={} fn=eldenring.exe+0x{:X} ak_result={}({result_name}) loaded_bank_id={} 0x{:08X}",
        bank_path.display(),
        bytes.len(),
        api.load_bank_memory_copy_rva,
        result,
        loaded_bank_id,
        loaded_bank_id
    );

    if let Some(ownership) = bank_ownership_for_load_result(result) {
        Ok(LoadBankResult {
            tracked_bank: TrackedBank {
                loaded_bank_id,
                ownership,
            },
            message,
        })
    } else {
        Err(message)
    }
}

unsafe fn unload_bank(api: RuntimeApi, loaded_bank_id: u32) -> String {
    let result = unsafe { (api.unload_bank)(loaded_bank_id, std::ptr::null()) };
    let result_name = ak_result_name(result);

    format!(
        "unload_bank fn=eldenring.exe+0x{:X} ak_result={}({result_name})",
        api.unload_bank_rva, result
    )
}

fn detect_player_conditions(
    hand_mode: HandMode,
    watched_sp_effect_ids: &HashSet<i32>,
) -> ConditionSnapshot {
    let mut snapshot = ConditionSnapshot::default();
    let Ok(world_chr_man) = (unsafe { WorldChrMan::instance() }) else {
        return snapshot;
    };
    let repo = unsafe { SoloParamRepository::instance() }.ok();

    for player in world_chr_man.player_chr_set.characters() {
        snapshot.player_count += 1;
        if let Some(repo) = repo
            && let Some(condition_id) =
                player_sp_atkcategory(player.chr_asm.as_ref(), hand_mode, repo)
        {
            snapshot.sp_atkcategories.insert(condition_id);
        }

        for entry in player.chr_ins.special_effect.entries() {
            if watched_sp_effect_ids.contains(&entry.param_id) {
                snapshot.sp_effect_ids.insert(entry.param_id);
            }
        }
    }

    snapshot
}

fn player_sp_atkcategory(
    chr_asm: &ChrAsm,
    hand_mode: HandMode,
    repo: &SoloParamRepository,
) -> Option<i32> {
    let hand = active_hand(hand_mode, chr_asm);
    let slot_index = active_weapon_slot_index(chr_asm, hand)?;
    let param_id = chr_asm.equipment_param_ids.get(slot_index).copied()?;
    if param_id <= 0 {
        return None;
    }

    let weapon = repo.get::<EquipParamWeapon>(((param_id as u32) / 100) * 100)?;
    Some(weapon.sp_atkcategory() as i32)
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use pelite::pe64::PeFile;

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
    fn runtime_symbols_resolve_from_configured_game_executable() {
        let Some(path) = std::env::var_os("ER_GAME_EXE").map(PathBuf::from) else {
            eprintln!("ER_GAME_EXE is not set; static game compatibility check skipped");
            return;
        };

        let bytes = fs::read(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        let pe = PeFile::from_bytes(&bytes)
            .unwrap_or_else(|err| panic!("failed to parse {}: {err:?}", path.display()));
        let register_task_rva = resolve_register_task_rva(pe)
            .unwrap_or_else(|err| panic!("register_task compatibility check failed: {err}"));
        let load_bank_memory_copy_rva = resolve_export_rva(pe, LOAD_BANK_MEMORY_COPY_EXPORT)
            .unwrap_or_else(|err| panic!("LoadBankMemoryCopy compatibility check failed: {err}"));
        let unload_bank_rva = resolve_export_rva(pe, UNLOAD_BANK_EXPORT)
            .unwrap_or_else(|err| panic!("UnloadBank compatibility check failed: {err}"));
        let text = pe
            .section_headers()
            .by_name(".text")
            .and_then(|section| pe.get_section_bytes(section).ok())
            .expect(".text section missing or malformed");
        let fd4_singleton_candidates = from_singleton::find::fd4_singleton_pat_iter(text).count();
        assert!(
            fd4_singleton_candidates > 0,
            "FD4 singleton reflection signature produced no candidates"
        );
        for name in [
            b"CSTask\0".as_slice(),
            b"WorldChrMan\0",
            b"SoloParamRepository\0",
        ] {
            assert!(
                bytes.windows(name.len()).any(|window| window == name),
                "required DLRF singleton name {} was not found",
                String::from_utf8_lossy(&name[..name.len() - 1])
            );
        }

        assert_ne!(register_task_rva, 0);
        assert_ne!(load_bank_memory_copy_rva, 0);
        assert_ne!(unload_bank_rva, 0);
        assert_ne!(load_bank_memory_copy_rva, unload_bank_rva);
        println!(
            "compatible executable={} register_task=0x{register_task_rva:X} LoadBankMemoryCopy=0x{load_bank_memory_copy_rva:X} UnloadBank=0x{unload_bank_rva:X} fd4_singleton_candidates={fd4_singleton_candidates} required_singleton_names=ok",
            path.display(),
        );
    }
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

fn bank_ownership_for_load_result(result: i32) -> Option<BankOwnership> {
    match result {
        AK_SUCCESS => Some(BankOwnership::Loader),
        AK_BANK_ALREADY_LOADED => Some(BankOwnership::External),
        _ => None,
    }
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
        69 => "AK_BankAlreadyLoaded",
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
