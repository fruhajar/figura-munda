//! The JNI bridge.
//!
//! **Not compiled in the environment this was written in.** There is no Android
//! NDK here, so this is the interface worked out in full rather than a thing
//! that has been seen to run. Every function is a thin wrapper over
//! [`crate::decide`], which is tested; the untested part is the marshalling.
//!
//! A session holds the profile and the journal across calls, so a sweep over a
//! thousand photographs does not re-parse its own bookkeeping a thousand times.
//! The handle is a leaked `Box` returned as a `jlong`, which is the ordinary way
//! to do this and is safe as long as `close` is called once; the Kotlin side
//! does that in a `finally`.

use crate::decide::{self, Item, Plan, Resolution, Skip};
use crate::journal::Journal;
use crate::sweep::{self, Conditions, Report};
use fm_core::profile::Profile;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jbyteArray, jint, jlong};
use jni::JNIEnv;

/// Codes returned by `plan`. Mirrored in `Native.kt`.
const PLAN_EXAMINE: jint = 0;
const PLAN_SETTLING: jint = 1;
const PLAN_DONE: jint = 2;
const PLAN_GIVEN_UP: jint = 3;

pub struct Session {
    profile: Profile,
    journal: Journal,
    report: Report,
    /// Why the last `resolve` produced nothing to write.
    last: Option<Resolution>,
    /// Every id the host has listed this sweep, so the journal can forget the
    /// rest at the end.
    present: Vec<String>,
}

fn session<'a>(handle: jlong) -> &'a mut Session {
    // Safe for a handle this module produced and has not closed.
    unsafe { &mut *(handle as *mut Session) }
}

fn string(env: &mut JNIEnv, s: &JString) -> String {
    env.get_string(s).map(|v| v.into()).unwrap_or_default()
}

fn item(id: String, name: String, bytes: i64, mtime: i64, ext: String) -> Item {
    Item {
        id,
        name,
        bytes: bytes.max(0) as u64,
        mtime,
        ext: (!ext.is_empty()).then_some(ext.to_lowercase()),
    }
}

#[no_mangle]
pub extern "system" fn Java_dev_figura_munda_Native_open(
    mut env: JNIEnv,
    _class: JClass,
    profile_toml: JString,
    journal_json: JString,
) -> jlong {
    let profile = Profile::from_toml(&string(&mut env, &profile_toml)).unwrap_or_else(|_| {
        // An unreadable profile means no sweep, not a sweep under assumptions
        // the user never agreed to.
        let mut fallback = Profile::standard();
        fallback.runner.enabled = false;
        fallback
    });
    let journal: Journal =
        serde_json::from_str(&string(&mut env, &journal_json)).unwrap_or_default();
    let session = Box::new(Session {
        profile,
        journal,
        report: Report::default(),
        last: None,
        present: Vec::new(),
    });
    Box::into_raw(session) as jlong
}

/// Empty means a sweep is owed. Anything else is the sentence to show.
#[no_mangle]
pub extern "system" fn Java_dev_figura_munda_Native_due<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    charging: jint,
    battery: jint,
    now: jlong,
) -> JString<'local> {
    let s = session(handle);
    let conditions = Conditions {
        charging: charging != 0,
        battery: battery.clamp(0, 100) as u8,
        now,
    };
    let text = match sweep::due(&s.profile, &s.journal, conditions) {
        Ok(()) => String::new(),
        Err(declined) => declined.sentence(),
    };
    env.new_string(text).expect("a short sentence always allocates")
}

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_dev_figura_munda_Native_plan(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    id: JString,
    name: JString,
    bytes: jlong,
    mtime: jlong,
    ext: JString,
    now: jlong,
) -> jint {
    let s = session(handle);
    let it = item(
        string(&mut env, &id),
        string(&mut env, &name),
        bytes,
        mtime,
        string(&mut env, &ext),
    );
    s.present.push(it.id.clone());
    let conditions = Conditions { charging: true, battery: 100, now };
    match decide::plan(&it, &s.profile, &s.journal, conditions) {
        Plan::Examine => PLAN_EXAMINE,
        Plan::Pass(Skip::StillSettling) => {
            s.report.skipped_recent += 1;
            PLAN_SETTLING
        }
        Plan::Pass(Skip::AlreadyDone) => {
            s.report.skipped_seen += 1;
            PLAN_DONE
        }
        Plan::Pass(Skip::GivenUp) => {
            s.report.skipped_seen += 1;
            PLAN_GIVEN_UP
        }
    }
}

/// The cleaned bytes, or an empty array when there is nothing to write. Ask
/// `outcome` why.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_dev_figura_munda_Native_resolve(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    id: JString,
    name: JString,
    bytes: jlong,
    mtime: jlong,
    ext: JString,
    data: JByteArray,
) -> jbyteArray {
    let s = session(handle);
    let it = item(
        string(&mut env, &id),
        string(&mut env, &name),
        bytes,
        mtime,
        string(&mut env, &ext),
    );
    let input = env.convert_byte_array(&data).unwrap_or_default();
    s.report.examined += 1;

    let resolution = decide::resolve(&it, &input, &s.profile);
    let out = match &resolution {
        Resolution::Write { data, .. } => {
            s.report.cleaned += 1;
            s.report.bytes_saved += input.len() as i64 - data.len() as i64;
            data.clone()
        }
        Resolution::AlreadyClean => {
            s.report.already_clean += 1;
            Vec::new()
        }
        Resolution::Refused(_) => {
            s.report.refused += 1;
            Vec::new()
        }
        Resolution::Failed(_) => {
            s.report.failed += 1;
            Vec::new()
        }
    };
    s.last = Some(resolution);
    env.byte_array_from_slice(&out)
        .map(|a| a.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// Why the last `resolve` produced nothing. Empty when it produced bytes.
#[no_mangle]
pub extern "system" fn Java_dev_figura_munda_Native_outcome<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) -> JString<'local> {
    let s = session(handle);
    let text = match &s.last {
        Some(Resolution::Write { .. }) | None => String::new(),
        Some(Resolution::AlreadyClean) => "already clean".to_string(),
        Some(Resolution::Refused(why)) => format!("not touched: {why}"),
        Some(Resolution::Failed(why)) => format!("failed: {why}"),
    };
    env.new_string(text).expect("a short ASCII string always allocates")
}

/// Write down what the host actually wrote. `wrote_bytes` below zero means it
/// wrote nothing, which is recorded as a failure rather than a success.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_dev_figura_munda_Native_record(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    id: JString,
    name: JString,
    bytes: jlong,
    mtime: jlong,
    ext: JString,
    wrote_bytes: jlong,
    wrote_mtime: jlong,
    now: jlong,
) {
    let s = session(handle);
    let it = item(
        string(&mut env, &id),
        string(&mut env, &name),
        bytes,
        mtime,
        string(&mut env, &ext),
    );
    let conditions = Conditions { charging: true, battery: 100, now };
    let resolution = s.last.take().unwrap_or(Resolution::Failed(
        "the host recorded a file it never resolved".to_string(),
    ));
    let wrote = (wrote_bytes >= 0).then_some((wrote_bytes as u64, wrote_mtime));
    decide::record(&mut s.journal, &it, &s.profile, conditions, &resolution, wrote);
}

/// The journal as JSON, for the host to persist. Called after every file: the
/// system will kill a background task partway, and a journal that only existed
/// at the end would mean starting over.
#[no_mangle]
pub extern "system" fn Java_dev_figura_munda_Native_journal<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) -> JString<'local> {
    let s = session(handle);
    let text = serde_json::to_string(&s.journal).unwrap_or_else(|_| "{}".to_string());
    env.new_string(text).expect("serialised JSON always allocates")
}

/// Finish the sweep: stamp the time, forget files the host no longer lists.
#[no_mangle]
pub extern "system" fn Java_dev_figura_munda_Native_finish(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    now: jlong,
) {
    let s = session(handle);
    s.journal.last_sweep = Some(now);
    let present = std::mem::take(&mut s.present);
    s.journal.forget_absent(&present);
}

/// The line for the notification.
#[no_mangle]
pub extern "system" fn Java_dev_figura_munda_Native_report<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) -> JString<'local> {
    let s = session(handle);
    env.new_string(s.report.sentence()).expect("a report line always allocates")
}

#[no_mangle]
pub extern "system" fn Java_dev_figura_munda_Native_close(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle != 0 {
        // Retake ownership and drop it.
        let _ = unsafe { Box::from_raw(handle as *mut Session) };
    }
}
