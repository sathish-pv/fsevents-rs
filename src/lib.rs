#[macro_use]
extern crate napi_derive;

use std::ffi::{CStr, c_char, c_void};
use std::{ptr, thread};

use fsevent_sys as fs;
use fsevent_sys::{core_foundation as cf, *};
use napi::threadsafe_function::{ErrorStrategy, ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::{CallContext, JsNumber, JsObject, JsString, bindgen_prelude::*};

struct FseInstance {
    runloop: Option<cf::CFRunLoopRef>,
    callback: Option<ThreadsafeFunction<Event, ErrorStrategy::CalleeHandled>>,
    handle: Option<thread::JoinHandle<Result<()>>>,
}

#[napi]
#[derive(Debug)]
pub struct Event {
    pub event_id: i64,
    pub flag: u32,
    pub path: String,
}
fn default_stream_context(instance: &ThreadsafeFunction<Event>) -> fs::FSEventStreamContext {
    let instance = instance as *const ThreadsafeFunction<Event>;
    fs::FSEventStreamContext {
        version: 0,
        info: instance as *mut c_void,
        retain: None,
        release: None,
        copy_description: None,
    }
}

extern "C" fn callback(
    _stream_ref: fs::FSEventStreamRef,
    info: *mut c_void,
    num_events: usize,                               // size_t numEvents
    event_paths: *mut c_void,                        // void *eventPaths
    event_flags: *const fs::FSEventStreamEventFlags, // const FSEventStreamEventFlags eventFlags[]
    event_ids: *const fs::FSEventStreamEventId,      // const FSEventStreamEventId eventIds[]
) {
    unsafe {
        let event_paths = event_paths as *const *const c_char;
        let callback = info as *mut ThreadsafeFunction<Event>;

        for pos in 0..num_events {
            let path = CStr::from_ptr(*event_paths)
                .to_str()
                .expect("Invalid UTF8 string.");
            let flag = *event_flags.add(pos);
            let event_id = *event_ids.add(pos);

            let event = Event {
                event_id: event_id.try_into().unwrap(),
                flag,
                path: path.to_string(),
            };

            let js_callback = callback.as_ref().unwrap();

            let status = js_callback.call(Ok(event), ThreadsafeFunctionCallMode::Blocking);

            assert_eq!(status, Status::Ok, "Error calling JS callback");
        }
    }
}

unsafe impl Send for FseInstance {}

fn build_native_paths(path: &str) -> Result<cf::CFMutableArrayRef> {
    let native_paths =
        unsafe { cf::CFArrayCreateMutable(cf::kCFAllocatorDefault, 0, &cf::kCFTypeArrayCallBacks) };

    if native_paths.is_null() {
        Err(Error::from_reason("Unable to create CFMutableArrayRef"))
    } else {
        unsafe {
            let mut err = ptr::null_mut();
            let cf_path = cf::str_path_to_cfstring_ref(path, &mut err);
            if !err.is_null() {
                let cf_str = cf::CFCopyDescription(err as cf::CFRef);
                let mut buf = [0; 1024];
                cf::CFStringGetCString(
                    cf_str,
                    buf.as_mut_ptr(),
                    buf.len() as cf::CFIndex,
                    cf::kCFStringEncodingUTF8,
                );
                return Err(Error::from_reason(
                    CStr::from_ptr(buf.as_ptr())
                        .to_str()
                        .unwrap_or("Unknown error")
                        .to_string(),
                ));
            } else {
                cf::CFArrayAppendValue(native_paths, cf_path);
                cf::CFRelease(cf_path);
            }
        }

        Ok(native_paths)
    }
}

#[js_function(3)]
fn fse_start(args: CallContext) -> Result<External<FseInstance>> {
    let path = args.get::<JsString>(0)?.into_utf8()?.as_str()?.to_string();
    let since = args.get::<JsNumber>(1)?.get_double()? as i64;
    let callback_js = args.get::<JsFunction>(2)?;

    let mut tsfn = callback_js
        .create_threadsafe_function(0, |ctx| {
            let env = ctx.env;
            let event: Event = ctx.value;
            let event_id = event.event_id;
            let flag = event.flag;
            let path = event.path;
            let args = (
                env.create_string(&path).unwrap(),
                env.create_uint32(flag).unwrap(),
                env.create_int64(event_id).unwrap(),
            );
            let args = args.into_vec(env.raw()).unwrap();
            Ok(args)
        })?;

    tsfn.refer(args.env)?;

    let tscallback = tsfn.clone();
    let mut instance = FseInstance {
        callback: Some(tsfn),
        runloop: None,

        handle: None,
    };

    let (ret_tx, ret_rx) = std::sync::mpsc::channel();
    struct CFRunLoopSendWrapper(cf::CFRunLoopRef);

    unsafe impl Send for CFRunLoopSendWrapper {}

    let handle = thread::Builder::new()
        .name("c eventloop".to_owned())
        .spawn(move || -> Result<()> {
            unsafe {
                let stream_context = default_stream_context(&tscallback);
                let paths = build_native_paths(&path)?;

                let stream = fs::FSEventStreamCreate(
                    cf::kCFAllocatorDefault,
                    callback,
                    &stream_context,
                    paths,
                    fs::kFSEventStreamEventIdSinceNow,
                    since as cf::CFAbsoluteTime,
                    kFSEventStreamCreateFlagNone
                        | kFSEventStreamCreateFlagWatchRoot
                        | kFSEventStreamCreateFlagFileEvents,
                );

                let runloop = CFRunLoopSendWrapper(cf::CFRunLoopGetCurrent());
                ret_tx.send(runloop).expect("unabe to send CFRunLoopRef");

                fs::FSEventStreamScheduleWithRunLoop(
                    stream,
                    cf::CFRunLoopGetCurrent(),
                    cf::kCFRunLoopDefaultMode,
                );

                fs::FSEventStreamStart(stream);
                cf::CFRunLoopRun();
                dbg!("CFRunLoopRun finished");

                fs::FSEventStreamFlushSync(stream);
                fs::FSEventStreamStop(stream);
                Ok(())
            }
        })
        .unwrap();

    instance.runloop = Some(ret_rx.recv().unwrap().0);
    instance.handle = Some(handle);

    Ok(instance.into())
}

#[js_function(1)]
fn fse_stop(args: CallContext) -> Result<()> {
    let mut instance = args.get::<External<FseInstance>>(0)?;

    if let Some(mut callback) = instance.callback.take() {
        callback.unref(args.env)?;
        callback.abort()?;
    }

    if let Some(handle) = instance.handle.take() {
        dbg!("Stopping thread");
        unsafe {
            cf::CFRunLoopStop(instance.runloop.unwrap());
        }
        handle.join().unwrap()?;
    }

    Ok(())
}

fn fse_flags(env: Env) -> Result<JsObject> {
    let mut flags = env.create_object()?;

    let since_now = env.create_int64(kFSEventStreamEventIdSinceNow as _)?;

    flags.set_named_property("sinceNow", since_now)?;
    Ok(flags)
}

fn fse_constants(env: Env) -> Result<JsObject> {
    let mut constants = env.create_object()?;
    use paste::paste;
    macro_rules! create_constant {
        ($name:ident) => {
            paste! {
                let constant = env.create_uint32([<kFSEventStreamEventFlag $name>] as _)?;
                constants.set_named_property(stringify!($name), constant)?;
            }
        };
    }

    create_constant!(None);
    create_constant!(MustScanSubDirs);
    create_constant!(UserDropped);
    create_constant!(KernelDropped);
    create_constant!(EventIdsWrapped);
    create_constant!(HistoryDone);
    create_constant!(RootChanged);
    create_constant!(Mount);
    create_constant!(Unmount);
    create_constant!(ItemCreated);
    create_constant!(ItemRemoved);
    create_constant!(ItemInodeMetaMod);
    create_constant!(ItemRenamed);
    create_constant!(ItemModified);
    create_constant!(ItemFinderInfoMod);
    create_constant!(ItemChangeOwner);
    create_constant!(ItemXattrMod);
    create_constant!(ItemIsFile);
    create_constant!(ItemIsDir);
    create_constant!(ItemIsSymlink);
    create_constant!(ItemIsHardlink);
    create_constant!(ItemIsLastHardlink);
    create_constant!(OwnEvent);
    create_constant!(ItemCloned);
    Ok(constants)
}

#[module_exports]
pub fn init(mut exports: JsObject, env: Env) -> Result<()> {
    exports.create_named_method("start", fse_start)?;
    exports.create_named_method("stop", fse_stop)?;
    exports.set_named_property("constants", fse_constants(env))?;
    exports.set_named_property("flags", fse_flags(env))?;

    Ok(())
}
