//! Configurable native SFX Bank loading. Resource operations run only in the file-stage callback.
use std::{
    collections::{HashMap, HashSet},
    ffi::c_void,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

use pelite::pe64::{Pe, PeView};
use windows::Win32::System::LibraryLoader::GetModuleHandleA;

#[cfg(test)]
use crate::target_banks_for_conditions;
use crate::{
    ConditionBankMapping, Config, append_log, condition_config, parse_bool,
    parse_condition_bank_mappings,
};
use shikifsrs_elden_ring::{
    TaskCallback, TaskPhase, Ww171File, current_sound_conditions::CurrentSoundConditions,
    current_tasks::CurrentTaskEngine, task_resident::ResidentCallbackGroupSlot,
};
use std::{
    cell::RefCell,
    thread::{self, ThreadId},
};
static TASKS: ResidentCallbackGroupSlot<2> = ResidentCallbackGroupSlot::new();
thread_local! { static READER: RefCell<Option<Result<CurrentSoundConditions,String>>> = const { RefCell::new(None) }; }
#[derive(Default)]
struct Demand {
    targets: HashSet<u32>,
    blocked: bool,
}

#[derive(Clone)]
pub(super) struct Settings {
    pub enabled: bool,
    weapon_mappings: Vec<ConditionBankMapping>,
    sp_effect_mappings: Vec<ConditionBankMapping>,
    ready_timeout_ms: u64,
    invalid: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            weapon_mappings: Vec::new(),
            sp_effect_mappings: Vec::new(),
            ready_timeout_ms: 10_000,
            invalid: false,
        }
    }
}

impl Settings {
    pub fn parse_entry(&mut self, key: &str, value: &str) {
        match key {
            "sfx_enabled" | "experimental_sfx_enabled" => match parse_bool(value) {
                Some(enabled) => self.enabled = enabled,
                None => self.invalid = true,
            },
            "sfx_condition_bank_mapping" | "sfx_sp_atkcategory_bank_mapping" => {
                parse_mapping(value, &mut self.weapon_mappings, &mut self.invalid);
            }
            "sfx_sp_effect_bank_mapping" | "sfx_speffect_bank_mapping" => {
                parse_mapping(value, &mut self.sp_effect_mappings, &mut self.invalid);
            }
            "sfx_ready_timeout_ms" => match value.parse::<u64>() {
                Ok(value @ 1000..=120_000) => self.ready_timeout_ms = value,
                _ => self.invalid = true,
            },
            _ => {}
        }
    }

    pub fn summary(&self) -> String {
        format!(
            "enabled={} weapon_mappings={} sp_effect_mappings={} ready_timeout_ms={} invalid={}",
            self.enabled,
            format_mappings(&self.weapon_mappings),
            format_mappings(&self.sp_effect_mappings),
            self.ready_timeout_ms,
            self.invalid,
        )
    }
}

fn format_mappings(mappings: &[ConditionBankMapping]) -> String {
    mappings
        .iter()
        .map(|mapping| format!("{}:{}", mapping.condition_id, mapping.bank_id))
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_mapping(value: &str, target: &mut Vec<ConditionBankMapping>, invalid: &mut bool) {
    let mappings = parse_condition_bank_mappings(value);
    if value.contains(',')
        || mappings.len() != 1
        || mappings[0].condition_id < 0
        || mappings[0].bank_id > 9999
    {
        *invalid = true;
    } else {
        target.extend(mappings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pelite::pe64::PeFile;

    #[derive(Default)]
    struct Fake {
        acquired: usize,
        released: usize,
        owners: usize,
        loader_owners: usize,
        ready: bool,
        empty: bool,
        missing: bool,
        unavailable: bool,
        lost: bool,
    }

    impl Backend for Fake {
        fn available(&mut self) -> Result<bool, String> {
            if self.lost {
                Err("identity changed".into())
            } else {
                Ok(!self.unavailable)
            }
        }
        fn acquire(&mut self, _bank: u32) -> Result<usize, String> {
            self.acquired += 1;
            if self.missing {
                return Err("missing".into());
            }
            self.loader_owners += 1;
            Ok(1)
        }
        fn inspect(&mut self, _handle: usize) -> Result<Observation, String> {
            Ok(Observation {
                state: if self.ready { 4 } else { 1 },
                references: (1 + self.owners + self.loader_owners) as u32,
                uses: (1 + self.owners + self.loader_owners) as i32,
                children: if self.empty { [0; 5] } else { [1; 5] },
            })
        }
        fn release(&mut self, _handle: usize) -> Result<(), String> {
            assert!(self.loader_owners > 0);
            self.loader_owners -= 1;
            self.released += 1;
            Ok(())
        }
    }

    fn demand() -> HashSet<u32> {
        HashSet::from([2120])
    }

    #[test]
    fn settings_default_off_and_repeated_mappings_union() {
        let mut settings = Settings::default();
        assert!(!settings.enabled);
        settings.parse_entry("sfx_enabled", "true");
        settings.parse_entry("sfx_condition_bank_mapping", "86:2120");
        settings.parse_entry("sfx_condition_bank_mapping", "87:2120");
        assert!(settings.enabled && !settings.invalid);
        let banks = target_banks_for_conditions(
            &HashSet::from([86, 87]),
            &settings.weapon_mappings,
            &HashSet::new(),
            &[],
        );
        assert_eq!(banks, demand());
        assert!(
            target_banks_for_conditions(
                &HashSet::from([85]),
                &settings.weapon_mappings,
                &HashSet::new(),
                &[]
            )
            .is_empty()
        );
    }

    #[test]
    fn weapon_and_sp_effect_sfx_mappings_form_one_bank_union() {
        let mut settings = Settings::default();
        settings.parse_entry("experimental_sfx_enabled", "true");
        settings.parse_entry("sfx_condition_bank_mapping", "86:2120");
        settings.parse_entry("sfx_sp_effect_bank_mapping", "190001:2120");
        settings.parse_entry("sfx_sp_effect_bank_mapping", "190002:2130");
        assert!(settings.enabled && !settings.invalid);
        assert_eq!(
            target_banks_for_conditions(
                &HashSet::from([86]),
                &settings.weapon_mappings,
                &HashSet::from([190001, 190002]),
                &settings.sp_effect_mappings,
            ),
            HashSet::from([2120, 2130])
        );
    }

    #[test]
    fn timeout_is_bounded_and_configurable() {
        let mut settings = Settings::default();
        settings.parse_entry("sfx_ready_timeout_ms", "30000");
        assert_eq!(settings.ready_timeout_ms, 30_000);
        for value in ["999", "120001", "invalid"] {
            let mut settings = Settings::default();
            settings.parse_entry("sfx_ready_timeout_ms", value);
            assert!(settings.invalid, "{value}");
        }
    }

    #[test]
    fn malformed_mapping_disables_experiment() {
        for value in ["bad", "-1:2120", "86:10000", "86:2120,bad", "86:-1"] {
            let mut settings = Settings::default();
            settings.parse_entry("sfx_condition_bank_mapping", value);
            assert!(settings.invalid, "{value}");
        }
    }

    #[test]
    fn repeated_polls_acquire_once_and_keep_external_owners() {
        let mut driver = Driver::new(
            Fake {
                ready: true,
                owners: 2,
                ..Default::default()
            },
            10_000,
        );
        let first = driver.tick(&demand(), 0);
        assert!(first.iter().any(|line| line.contains("resources_ready")));
        for time in 1..100 {
            assert!(driver.tick(&demand(), time).is_empty());
        }
        assert_eq!(driver.backend.acquired, 1);
        driver.tick(&HashSet::new(), 101);
        driver.tick(&HashSet::new(), 102);
        assert_eq!(driver.backend.released, 1);
        assert_eq!(driver.backend.owners, 2);
        assert_eq!(driver.backend.loader_owners, 0);
    }

    #[test]
    fn ten_cycles_are_balanced() {
        let mut driver = Driver::new(
            Fake {
                ready: true,
                ..Default::default()
            },
            10_000,
        );
        for cycle in 0..10 {
            driver.tick(&demand(), cycle * 2);
            driver.tick(&HashSet::new(), cycle * 2 + 1);
        }
        assert_eq!(driver.backend.acquired, 10);
        assert_eq!(driver.backend.released, 10);
        assert_eq!(driver.backend.loader_owners, 0);
    }

    #[test]
    fn timeout_keeps_one_request_and_can_complete_later() {
        let mut driver = Driver::new(Fake::default(), 10_000);
        driver.tick(&demand(), 0);
        assert!(driver.tick(&demand(), 9999).is_empty());
        assert!(
            driver
                .tick(&demand(), 10000)
                .iter()
                .any(|line| line.contains("timeout"))
        );
        assert!(driver.tick(&demand(), 10001).is_empty());
        driver.backend.ready = true;
        assert!(
            driver
                .tick(&demand(), 20000)
                .iter()
                .any(|line| line.contains("resources_ready"))
        );
        assert_eq!(driver.backend.acquired, 1);
        assert_eq!(driver.backend.released, 0);
    }

    #[test]
    fn ready_file_without_fxr_is_not_resource_ready() {
        let mut driver = Driver::new(
            Fake {
                ready: true,
                empty: true,
                ..Default::default()
            },
            10_000,
        );
        assert!(
            !driver
                .tick(&demand(), 0)
                .iter()
                .any(|line| line.contains("resources_ready"))
        );
        assert!(
            driver
                .tick(&demand(), 10000)
                .iter()
                .any(|line| line.contains("timeout"))
        );
    }

    #[test]
    fn release_pending_request_once_without_cancelling_baseline() {
        let mut driver = Driver::new(Fake::default(), 10_000);
        driver.tick(&demand(), 0);
        driver.tick(&HashSet::new(), 1);
        assert_eq!(driver.backend.released, 1);
        assert!(driver.held.is_empty());
        assert!(!driver.stopped);
    }

    #[test]
    fn missing_request_is_latched_until_demand_ends() {
        let mut driver = Driver::new(
            Fake {
                missing: true,
                ..Default::default()
            },
            10_000,
        );
        assert!(
            driver
                .tick(&demand(), 0)
                .iter()
                .any(|line| line.contains("request failed"))
        );
        driver.tick(&demand(), 20000);
        assert_eq!(driver.backend.acquired, 1);
        driver.tick(&HashSet::new(), 20001);
        driver.tick(&demand(), 20002);
        assert_eq!(driver.backend.acquired, 2);
        assert_eq!(driver.backend.released, 0);
    }

    #[test]
    fn unavailable_initial_manager_waits_but_lost_identity_stops() {
        let mut driver = Driver::new(
            Fake {
                unavailable: true,
                ..Default::default()
            },
            10_000,
        );
        driver.tick(&demand(), 0);
        assert_eq!(driver.backend.acquired, 0);
        driver.backend.unavailable = false;
        driver.tick(&demand(), 1);
        driver.backend.lost = true;
        assert!(
            driver
                .tick(&HashSet::new(), 2)
                .iter()
                .any(|line| line.contains("STOPPED"))
        );
        driver.backend.lost = false;
        driver.tick(&HashSet::new(), 3);
        assert_eq!(driver.backend.released, 0);
        assert!(driver.stopped);
    }

    #[test]
    fn fixed_current_profile_rejects_previous_and_unknown_identity() {
        let new = select_profile(0x6a96b418, 0x5e0da00).unwrap();
        assert_eq!(new.code[0].1, 0xd90d60);
        assert_eq!(new.code[1].1, 0xd91090);
        assert!(select_profile(0x69e9c9b9, 0x5e09600).is_err());
        assert!(select_profile(0x6a96b418, 0x5e09600).is_err());
        assert!(select_profile(0, 0).is_err());
    }

    #[test]
    fn configured_game_image_matches_and_tampering_is_rejected() {
        let Some(path) = std::env::var_os("ER_GAME_EXE") else {
            eprintln!("SKIPPED SFX executable probe: set ER_GAME_EXE to run");
            return;
        };
        let mut bytes = std::fs::read(path).expect("read configured executable");
        let pe = PeFile::from_bytes(&bytes).expect("PE64");
        let profile = validate_image(pe).expect("SFX native code matches inspected executable");
        for &(name, rva, _, _) in profile.code {
            let offset = PeFile::from_bytes(&bytes)
                .unwrap()
                .rva_to_file_offset(rva)
                .unwrap();
            bytes[offset] ^= 1;
            assert!(
                validate_image(PeFile::from_bytes(&bytes).unwrap())
                    .unwrap_err()
                    .contains("fingerprint mismatch"),
                "{name}"
            );
            bytes[offset] ^= 1;
        }
        println!(
            "SFX {} fingerprints PASS; modified acquire function rejected",
            profile.version
        );
    }
}

pub(super) fn start(
    bytes: Arc<[u8]>,
    base: u64,
    log: Option<PathBuf>,
    config: &Config,
) -> Result<(), String> {
    if config.sfx.invalid
        || (config.sfx.weapon_mappings.is_empty() && config.sfx.sp_effect_mappings.is_empty())
    {
        append_log(&log, "sfx disabled: invalid settings or no mappings");
        return Ok(());
    }
    let file = Ww171File::verify(&bytes).map_err(|e| e.to_string())?;
    let resource_phase = file
        .phase("FileStepUpdate_Begin")
        .map_err(|e| e.to_string())?;
    let engine =
        unsafe { CurrentTaskEngine::bind(bytes.clone(), base) }.map_err(|e| e.to_string())?;
    let demand = Arc::new(Mutex::new(Demand::default()));
    let publisher = demand.clone();
    let condition_log = log.clone();
    let reader_config = condition_config(
        config.condition_hand,
        &config.sfx.weapon_mappings,
        &config.sfx.sp_effect_mappings,
    );
    let reader_bytes = bytes.clone();
    let poll = config.poll_every_frames.max(1);
    let mut frame = 0u64;
    let condition = TaskCallback::new(move |_| {
        frame = frame.saturating_add(1);
        if !frame.is_multiple_of(u64::from(poll)) {
            return;
        }
        let result = READER.with(|slot| {
            let mut state = slot.borrow_mut();
            if state.is_none() {
                *state = Some(
                    unsafe {
                        CurrentSoundConditions::bind(
                            reader_bytes.clone(),
                            base,
                            None,
                            reader_config.clone(),
                        )
                    }
                    .map_err(|e| e.to_string()),
                );
            }
            match state.as_ref().unwrap() {
                Ok(reader) => reader.targets(1).map_err(|e| e.to_string()),
                Err(error) => Err(error.clone()),
            }
        });
        let Ok(mut state) = publisher.lock() else {
            return;
        };
        if state.blocked {
            return;
        }
        match result {
            Ok(Some(targets)) => {
                let next = targets.into_iter().collect::<HashSet<_>>();
                if state.targets != next {
                    append_log(&condition_log, &format!("sfx desired bank IDs={next:?}"));
                    state.targets = next;
                }
            }
            Ok(None) => {
                state.targets.clear();
            }
            Err(error) => {
                state.blocked = true;
                append_log(
                    &condition_log,
                    &format!(
                        "sfx condition ERROR={error}; retained resources, no subsequent native calls"
                    ),
                );
            }
        }
    });
    let timeout = config.sfx.ready_timeout_ms;
    let mut driver: Option<Driver<Native>> = None;
    let mut original_thread: Option<ThreadId> = None;
    let started = Instant::now();
    let mut stopped = false;
    let resource = TaskCallback::new(move |_| {
        if stopped {
            return;
        }
        let current_thread = thread::current().id();
        if original_thread.is_some_and(|id| id != current_thread) {
            stopped = true;
            append_log(
                &log,
                "sfx callback thread changed; retained references, stopped",
            );
            return;
        }
        original_thread = Some(current_thread);
        let Ok(state) = demand.lock() else {
            stopped = true;
            return;
        };
        if state.blocked {
            stopped = true;
            return;
        }
        let current = state.targets.clone();
        drop(state);
        if driver.is_none() {
            match unsafe { Native::resolve() } {
                Ok(backend) => {
                    append_log(
                        &log,
                        &format!(
                            "sfx owned resource adapter ER{}; phase/lifetime experimental",
                            backend.profile.version
                        ),
                    );
                    driver = Some(Driver::new(backend, timeout));
                }
                Err(error) => {
                    append_log(&log, &format!("sfx adapter rejected: {error}"));
                    stopped = true;
                    return;
                }
            }
        }
        let now = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        for event in driver.as_mut().unwrap().tick(&current, now) {
            append_log(&log, &format!("sfx {event}"));
        }
    });
    let installed = unsafe {
        TASKS.install(
            engine,
            [TaskPhase::FRAME_BEGIN, resource_phase],
            [condition, resource],
        )
    }
    .map_err(|e| e.to_string())?;
    let control = installed.registration.map_err(|e| e.to_string())?;
    if !control.registered_all() {
        control.stop_callbacks();
        return Err(
            "partial SFX task registration retained; no resource callback registered".into(),
        );
    }
    Ok(())
}

// Current-version native SFX code belongs to the original consumer. It is retained
// with its original raw guards, independently from SDK task/condition replacement.
const CODE_271: &[(&str, u32, usize, u64)] = &[
    ("path_acquire", 0xd90d60, 0x208, 0xca1c34d3b16b4032),
    ("release_cap", 0xd91090, 0x56, 0x771e160290158dfe),
    ("ffxbnd_factory", 0x1f1d10, 0x123, 0x208b866c9be4c972),
    ("collector", 0xd93230, 0x298, 0x8d3372a36f113b7e),
    ("csfile_release", 0x26541e0, 0x94, 0x6402dc4629687677),
    ("reference_release", 0x2654280, 0x76, 0x9dbe686227bb6ef3),
    ("increment_use", 0x20f2b0, 0x7, 0xe3741b79a0cd5043),
    ("decrement_use", 0x20f320, 0x1e, 0x0092f536dab1d5f3),
];

#[derive(Debug)]
struct Profile {
    version: &'static str,
    timestamp: u32,
    image_size: u32,
    code: &'static [(&'static str, u32, usize, u64)],
}

const PROFILES: &[Profile] = &[Profile {
    version: "2.7.1.0",
    timestamp: 0x6a96b418,
    image_size: 0x5e0da00,
    code: CODE_271,
}];

fn select_profile(timestamp: u32, image_size: u32) -> Result<&'static Profile, String> {
    PROFILES
        .iter()
        .find(|p| p.timestamp == timestamp && p.image_size == image_size)
        .ok_or_else(|| "unsupported executable identity; SFX requires fixed ER 2.7.1.0".into())
}

fn fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn validate_image<'a>(pe: impl Pe<'a>) -> Result<&'static Profile, String> {
    let profile = select_profile(
        pe.file_header().TimeDateStamp,
        pe.optional_header().SizeOfImage,
    )?;
    for &(name, rva, size, expected) in profile.code {
        let bytes = pe
            .derva_slice::<u8>(rva, size)
            .map_err(|e| format!("{name}: {e:?}"))?;
        if fingerprint(bytes) != expected {
            return Err(format!(
                "native code fingerprint mismatch: {name} at {rva:#x}"
            ));
        }
    }
    Ok(profile)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Context {
    manager: usize,
    file: usize,
    sentinel: usize,
}

#[derive(Debug, Default)]
struct Observation {
    state: u8,
    references: u32,
    uses: i32,
    // Native child vector order: TPF, FLVER, FXR, FFXRESLIST, ANIBND.
    children: [usize; 5],
}

impl Observation {
    fn ready(&self) -> bool {
        self.state == 4 && self.children[2] > 0
    }
}

trait Backend {
    fn available(&mut self) -> Result<bool, String>;
    fn acquire(&mut self, bank: u32) -> Result<usize, String>;
    fn inspect(&mut self, handle: usize) -> Result<Observation, String>;
    fn release(&mut self, handle: usize) -> Result<(), String>;
}

type Acquire = unsafe extern "system" fn(usize, *const u16, *const c_void, bool) -> usize;
type Release = unsafe extern "system" fn(usize, usize);

struct Native {
    base: usize,
    profile: &'static Profile,
    context: Option<Context>,
    acquire: Acquire,
    release: Release,
}

impl Native {
    unsafe fn resolve() -> Result<Self, String> {
        let base = unsafe { GetModuleHandleA(None) }
            .map_err(|e| format!("module: {e:?}"))?
            .0 as usize;
        let pe = unsafe { PeView::module(base as *const u8) };
        let profile = validate_image(pe)?;
        Ok(Self {
            base,
            profile,
            context: None,
            acquire: unsafe {
                std::mem::transmute::<usize, Acquire>(base + profile.code[0].1 as usize)
            },
            release: unsafe {
                std::mem::transmute::<usize, Release>(base + profile.code[1].1 as usize)
            },
        })
    }

    // Raw native layouts are confined to this version-checked adapter. Its task stage is
    // experimental; volatile reads do not constitute synchronization with game workers.
    unsafe fn read<T: Copy>(address: usize) -> T {
        unsafe { (address as *const T).read_volatile() }
    }

    fn current_context(&self) -> Result<Option<Context>, String> {
        unsafe {
            let manager = Self::read::<usize>(self.base + 0x3d87a28);
            let file = Self::read::<usize>(self.base + 0x3d5f158);
            if manager == 0 || file == 0 {
                return Ok(None);
            }
            if Self::read::<usize>(manager) != self.base + 0x2bc31a0
                || Self::read::<usize>(file) != self.base + 0x29d4800
            {
                return Err("CSSfx/CSFile vtable mismatch".into());
            }
            let sentinel = Self::read::<usize>(manager + 0xb8);
            if sentinel == 0 {
                return Ok(None);
            }
            Ok(Some(Context {
                manager,
                file,
                sentinel,
            }))
        }
    }

    fn checked_context(&self) -> Result<Context, String> {
        let current = self.current_context()?;
        match (self.context, current) {
            (Some(expected), Some(actual)) if expected == actual => Ok(actual),
            _ => Err(
                "native manager identity lost/changed; retained handles cannot be released safely"
                    .into(),
            ),
        }
    }

    fn contains(&self, context: Context, handle: usize) -> Result<bool, String> {
        unsafe {
            let count = Self::read::<usize>(context.manager + 0xc0);
            if count > 4096 {
                return Err("invalid/unsupported CSSfx Bank-list size".into());
            }
            let mut node = Self::read::<usize>(context.sentinel);
            for _ in 0..=count {
                if node == context.sentinel {
                    return Ok(false);
                }
                if node == 0 {
                    return Err("null CSSfx Bank-list node".into());
                }
                if Self::read::<usize>(node + 0x10) == handle {
                    return Ok(true);
                }
                node = Self::read::<usize>(node);
            }
        }
        Err("CSSfx Bank-list traversal bound exceeded".into())
    }
}

impl Backend for Native {
    fn available(&mut self) -> Result<bool, String> {
        let current = self.current_context()?;
        if self.context.is_some() && self.context != current {
            return Err("CSSfx/CSFile identity changed; restart required".into());
        }
        self.context = current;
        Ok(current.is_some())
    }

    fn acquire(&mut self, bank: u32) -> Result<usize, String> {
        let context = self.checked_context()?;
        let path = format!("sfxbnd:/SfxBnd_c{bank:04}.ffxbnd")
            .encode_utf16()
            .chain([0])
            .collect::<Vec<_>>();
        let handle =
            unsafe { (self.acquire)(context.manager, path.as_ptr(), std::ptr::null(), false) };
        if handle == 0 {
            return Err("native request returned null; retry only after demand ends".into());
        }
        Ok(handle)
    }

    fn inspect(&mut self, handle: usize) -> Result<Observation, String> {
        let context = self.checked_context()?;
        if handle == 0 || !self.contains(context, handle)? {
            return Err(
                "retained cap is absent from CSSfx; no stale-pointer release attempted".into(),
            );
        }
        unsafe {
            if Self::read::<usize>(handle) != self.base + 0x29daae0 {
                return Err("retained cap is not the expected FfxbndFileCap".into());
            }
            let mut observation = Observation {
                state: Self::read(handle + 0x88),
                references: Self::read(handle + 0x58),
                uses: Self::read(handle + 0x150),
                ..Default::default()
            };
            if observation.references < 2 || observation.uses < 2 {
                return Err(format!("unverified reference ownership: {observation:?}"));
            }
            // Child vectors are populated asynchronously; inspect them only after Ready.
            if observation.state == 4 {
                for (index, offset) in [0x98, 0xb8, 0xd8, 0xf8, 0x118].into_iter().enumerate() {
                    let begin = Self::read::<usize>(handle + offset);
                    let end = Self::read::<usize>(handle + offset + 8);
                    let bytes = end
                        .checked_sub(begin)
                        .ok_or("invalid child vector ordering")?;
                    if (begin == 0 && end != 0) || bytes % 8 != 0 || bytes / 8 > 100_000 {
                        return Err("invalid child vector bounds".into());
                    }
                    observation.children[index] = bytes / 8;
                }
            }
            Ok(observation)
        }
    }

    fn release(&mut self, handle: usize) -> Result<(), String> {
        self.inspect(handle)?;
        let context = self.checked_context()?;
        unsafe { (self.release)(context.manager, handle) };
        Ok(())
    }
}

struct Entry {
    handle: usize,
    since_ms: u64,
    ready_logged: bool,
    timeout_logged: bool,
}

struct Driver<B> {
    backend: B,
    held: HashMap<u32, Entry>,
    failed: HashSet<u32>,
    ready_timeout_ms: u64,
    stopped: bool,
}

impl<B: Backend> Driver<B> {
    fn new(backend: B, ready_timeout_ms: u64) -> Self {
        Self {
            backend,
            held: HashMap::new(),
            failed: HashSet::new(),
            ready_timeout_ms,
            stopped: false,
        }
    }

    fn tick(&mut self, demand: &HashSet<u32>, now_ms: u64) -> Vec<String> {
        let mut events = Vec::new();
        if self.stopped {
            return events;
        }
        if let Err(error) = self.update(demand, now_ms, &mut events) {
            self.stopped = true;
            events.push(format!("STOPPED: {error}; no further native calls; possible retained references; restart required"));
        }
        events
    }

    fn update(
        &mut self,
        demand: &HashSet<u32>,
        now_ms: u64,
        events: &mut Vec<String>,
    ) -> Result<(), String> {
        self.failed.retain(|bank| demand.contains(bank));
        if demand.is_empty() && self.held.is_empty() {
            return Ok(());
        }
        if !self.backend.available()? {
            if !self.held.is_empty() {
                return Err("manager unavailable with outstanding handles".into());
            }
            return Ok(());
        }
        let inactive = self
            .held
            .keys()
            .filter(|bank| !demand.contains(bank))
            .copied()
            .collect::<Vec<_>>();
        for bank in inactive {
            let entry = &self.held[&bank];
            self.backend.release(entry.handle)?;
            events.push(format!(
                "bank={bank} released own reference cap={:#x}; physical collection is native",
                entry.handle
            ));
            self.held.remove(&bank);
        }
        for &bank in demand {
            if self.failed.contains(&bank) {
                continue;
            }
            if !self.held.contains_key(&bank) {
                match self.backend.acquire(bank) {
                    Ok(handle) => {
                        self.held.insert(
                            bank,
                            Entry {
                                handle,
                                since_ms: now_ms,
                                ready_logged: false,
                                timeout_logged: false,
                            },
                        );
                        events.push(format!("bank={bank} requested cap={handle:#x}"));
                    }
                    Err(error) => {
                        self.failed.insert(bank);
                        events.push(format!("bank={bank} request failed: {error}"));
                        continue;
                    }
                }
            }
            let entry = self.held.get_mut(&bank).expect("entry acquired above");
            let observation = self.backend.inspect(entry.handle)?;
            if observation.ready() && !entry.ready_logged {
                entry.ready_logged = true;
                events.push(format!("bank={bank} resources_ready elapsed_ms={} {observation:?}; visual test required", now_ms.saturating_sub(entry.since_ms)));
            } else if !entry.ready_logged
                && !entry.timeout_logged
                && now_ms.saturating_sub(entry.since_ms) >= self.ready_timeout_ms
            {
                entry.timeout_logged = true;
                events.push(format!(
                    "bank={bank} timeout {observation:?}; retaining one request, no retry/cancel"
                ));
            }
        }
        Ok(())
    }
}
