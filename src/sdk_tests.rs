//! Original consumer/SDK boundary scenarios. No native audio or game memory.
use super::*;
use shikifsrs_elden_ring::{
    character_equipment::CharacterEquipmentObservation,
    character_sets::CharacterIdentity,
    sound_bank_schedule::SoundBankSchedule,
    sound_banks::{BankLoad, BankLoadUncertainty, BankUnload, SoundBankEngine, SoundBankOwner},
};
use shikifsrs_runtime::{Error, Result as SdkResult};
use std::{collections::BTreeSet, sync::Mutex};
#[derive(Default)]
struct Trace {
    load: usize,
    unload: Vec<u32>,
}
struct Audio {
    trace: Arc<Mutex<Trace>>,
    result: BankLoad,
}
// SAFETY: Original synchronous fake creates no native obligation; the captured output models one attempt.
unsafe impl SoundBankEngine for Audio {
    unsafe fn load_memory_copy(&mut self, _: &[u8]) -> SdkResult<BankLoad> {
        self.trace.lock().unwrap().load += 1;
        Ok(self.result.clone())
    }
    unsafe fn unload(&mut self, id: u32) -> SdkResult<BankUnload> {
        self.trace.lock().unwrap().unload.push(id);
        Ok(BankUnload::Released)
    }
}
struct Source {
    fail: bool,
    reads: usize,
}
impl BankAssetSource for Source {
    fn read_bank(&mut self, _: u32) -> SdkResult<Vec<u8>> {
        self.reads += 1;
        if self.fail {
            Err(Error::NativeOperationRejected("missing synthetic bank"))
        } else {
            Ok(vec![0; 32])
        }
    }
}
#[test]
fn owned_left_right_and_auto_policy_maps_to_sdk_selected_slots() {
    for style in 0..4 {
        let copied = CharacterEquipmentObservation {
            character: CharacterIdentity {
                world_address: 8,
                address: 16,
                native_identifier: 24,
                epoch: 1,
            },
            assembly_address: 32,
            native_arm_style: style,
            native_selected_slots: [1, 2, 0, 0, 0, 0],
            parameter_ids: std::array::from_fn(|i| 1000 + i as i32),
        };
        assert_eq!(
            copied
                .selected_weapon(selection(HandMode::Left))
                .unwrap()
                .slot,
            2
        );
        assert_eq!(
            copied
                .selected_weapon(selection(HandMode::Right))
                .unwrap()
                .slot,
            5
        );
        assert_eq!(
            copied
                .selected_weapon(selection(HandMode::Auto))
                .unwrap()
                .slot,
            if style == 2 { 2 } else { 5 }
        );
    }
}
#[test]
fn startup_delay_is_not_duplicated_and_conditional_bypasses_static_preload() {
    let mut config = Config {
        sound_bank_ids: vec![2120],
        start_delay_ms: 5000,
        retry_every_frames: 1,
        ..Default::default()
    };
    assert_eq!(config.schedule().start_delay_ms, 0);
    assert_eq!(config.schedule().static_ids, vec![2120]);
    config.conditional_enabled = true;
    assert!(config.schedule().static_ids.is_empty());
    let conditions = condition_config(
        HandMode::Left,
        &[ConditionBankMapping {
            condition_id: 86,
            bank_id: 2120,
        }],
        &[ConditionBankMapping {
            condition_id: 190001,
            bank_id: 2130,
        }],
    );
    assert_eq!(conditions.selection, WeaponSelectionMode::Even);
    assert_eq!(conditions.weapon_mappings[0].condition_id, 86);
    assert_eq!(conditions.effect_mappings[0].bank_id, 2130);
}
#[test]
fn native_unknown_attempt_stops_retry_but_missing_source_consumes_budget_at_cadence_one() {
    for unknown in [false, true] {
        let trace = Arc::new(Mutex::new(Trace::default()));
        let audio = Audio {
            trace: trace.clone(),
            result: BankLoad::Uncertain(BankLoadUncertainty {
                native_result: 2,
                bank_id_output: 2120,
                postflight: Ok(()),
            }),
        };
        let mut owner = SoundBankOwner::new(audio, 16).unwrap();
        let config = Config {
            sound_bank_ids: vec![2120],
            retries_per_id: 2,
            retry_every_frames: 1,
            ..Default::default()
        };
        let mut schedule = SoundBankSchedule::new(config.schedule(), 0, &owner).unwrap();
        let mut source = Source {
            fail: !unknown,
            reads: 0,
        };
        for now in 0..5 {
            // SAFETY: Fake synchronous audio/source, serial original test execution.
            unsafe { schedule.startup_frame(now, &mut owner, &mut source) }.unwrap();
        }
        assert_eq!(source.reads, if unknown { 1 } else { 2 });
        assert_eq!(trace.lock().unwrap().load, usize::from(unknown));
        assert_eq!(owner.is_uncertain(), unknown);
    }
}
#[test]
fn shared_conditional_demands_do_not_unload_external_banks_and_missing_observation_keeps_owned() {
    for external in [false, true] {
        let trace = Arc::new(Mutex::new(Trace::default()));
        let audio = Audio {
            trace: trace.clone(),
            result: if external {
                BankLoad::AlreadyPresent(77)
            } else {
                BankLoad::Acquired(77)
            },
        };
        let mut owner = SoundBankOwner::new(audio, 16).unwrap();
        let config = Config {
            conditional_enabled: true,
            poll_every_frames: 1,
            ..Default::default()
        };
        let mut schedule = SoundBankSchedule::new(config.schedule(), 0, &owner).unwrap();
        let mut source = Source {
            fail: false,
            reads: 0,
        };
        for _ in 0..3 {
            // SAFETY: Fake original ownership and serial conditional phase.
            unsafe {
                schedule
                    .conditional_frame(&mut owner, &mut source, || Ok(Some(BTreeSet::from([2120]))))
            }
            .unwrap();
        }
        // SAFETY: Missing-world observation does not authorize unloading; no native input.
        unsafe { schedule.conditional_frame(&mut owner, &mut source, || Ok(None)) }.unwrap();
        assert_eq!(trace.lock().unwrap().load, 1);
        assert!(trace.lock().unwrap().unload.is_empty());
        // SAFETY: Explicit observed empty demand releases only this fake owner's acquired reference.
        unsafe {
            schedule.conditional_frame(&mut owner, &mut source, || Ok(Some(BTreeSet::new())))
        }
        .unwrap();
        assert_eq!(
            trace.lock().unwrap().unload,
            if external { vec![] } else { vec![77] }
        );
    }
}
