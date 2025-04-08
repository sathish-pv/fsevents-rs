// mod fsevents;
#[macro_use]
extern crate napi_derive;

use std::ffi::{c_char, c_void, CStr};
use std::ptr;

use fsevent_sys as fs;
use fsevent_sys::{core_foundation as cf, *};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{
    ErrorStrategy, ThreadSafeCallContext, ThreadsafeFunction, ThreadsafeFunctionCallMode,
};
use napi::*;

#[derive(Debug, Clone)]
pub struct FseEnvironment {
    pub runloop: Option<cf::CFRunLoopRef>,
}

fn fse_environment_create() -> External<FseEnvironment> {
    External::new(FseEnvironment { runloop: None })
}

struct FseInstance {
    path: String,
    // fseenv: FseEnvironment,
    // stream: Option<fs::FSEventStreamRef>,
    tsfn: Option<ThreadsafeFunction<Event>>,
}

use std::fmt::Debug;
impl Debug for FseInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FseInstance")
            .field("path", &self.path)
            .finish()
    }
}

#[napi]
#[derive(Debug)]
pub struct Event {
    pub event_id: i64,
    pub flag: u32,
    pub path: String,
}
fn default_stream_context(instance: FseInstance) -> fs::FSEventStreamContext {
    let instance = &instance as *const FseInstance;
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
        let instance = info as *mut FseInstance;

        for pos in 0..num_events {
            let path = CStr::from_ptr(*event_paths.add(pos))
                .to_str()
                .expect("Invalid UTF8 string.");
            let flag = *event_flags.add(pos);
            let event_id: i64 = *event_ids.add(pos) as _;

            let event = Event {
                event_id,
                flag,
                path: path.to_string(),
            };

            let js_callback = (*instance).tsfn.as_ref().unwrap();
            let status = js_callback.call(Ok(event), ThreadsafeFunctionCallMode::Blocking);
            println!("Error calling JS callback: {:?}", status);
        }
    }
}

#[js_function(1)]
fn fse_stop(args: CallContext) -> Result<()> {
    let instance = args.get::<External<FseEnvironment>>(0)?;
    // let instance = instance.as_ref();
    // let runloop = instance.runloop.take().unwrap();
    let runl = instance.as_ref().runloop;
    if let Some(runl) = runl {
        unsafe {
            fs::FSEventStreamStop(runl);
            cf::CFRunLoopStop(runl);
        }
    }
    Ok(())
}
unsafe impl Send for FseInstance {}

fn fse_dispatch_event(ctx: ThreadSafeCallContext<Event>) -> Result<Vec<*mut sys::napi_value__>> {
    let env = ctx.env;
    let event = ctx.value;
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
}

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

#[js_function(4)]
fn fse_start(args: CallContext) -> Result<External<FseEnvironment>> {
    let fseenv = args.get::<External<FseEnvironment>>(0)?;
    let path = args.get::<JsString>(1)?.into_utf8()?.as_str()?.to_string();
    // let since = args.get::<JsNumber>(2)?.get_double()? as i64;
    let callback_js = args.get::<JsFunction>(3)?;

    let tsfn: ThreadsafeFunction<Event, ErrorStrategy::CalleeHandled> =
        callback_js.create_threadsafe_function(0, fse_dispatch_event)?;

    let instance = FseInstance {
        path: path.clone(),
        tsfn: Some(tsfn),
        // fseenv: fseenv.clone(),
        // stream: None,
    };

    // let (ret_tx, ret_rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || -> Result<()> {
        unsafe {
            let paths = build_native_paths(&path)?;
            let stream_context = default_stream_context(instance);

            let stream = fs::FSEventStreamCreate(
                cf::kCFAllocatorDefault,
                callback,
                &stream_context,
                paths,
                fs::kFSEventStreamEventIdSinceNow,
                0.1 as cf::CFAbsoluteTime,
                kFSEventStreamCreateFlagNone
                    | kFSEventStreamCreateFlagWatchRoot
                    | kFSEventStreamCreateFlagFileEvents
                    | kFSEventStreamCreateFlagUseCFTypes,
            );

            // instance.stream = Some(stream);
            // let runloop = CFRunLoopSendWrapper(cf::CFRunLoopGetCurrent());
            /* ret_tx
                           .send(runloop)
                           .expect("unabe to send CFRunLoopRef");
            */
            fs::FSEventStreamScheduleWithRunLoop(
                stream,
                cf::CFRunLoopGetCurrent(),
                cf::kCFRunLoopDefaultMode,
            );

            fs::FSEventStreamStart(stream);
            cf::CFRunLoopRun();

            fs::FSEventStreamFlushSync(stream);
            fs::FSEventStreamStop(stream);
            Ok(())
        }
    });

    // instance.run = Some(ret_rx.recv().unwrap().0);

    // let env = External::new(fseenv);

    Ok(fseenv)
}

fn fse_flags(env: Env) -> Result<JsObject> {
    let mut flags = env.create_object().unwrap();

    let since_now = env
        .create_int64(kFSEventStreamEventIdSinceNow as _)
        .unwrap();

    flags.set_named_property("sinceNow", since_now)?;
    Ok(flags)
}

fn fse_constants(env: Env) -> Result<JsObject> {
    let mut constants = env.create_object().unwrap();

    macro_rules! create_constant {
        ($name:ident) => {
            let constant = env.create_uint32($name as _).unwrap();
            constants.set_named_property(stringify!($name), constant)?;
        };
    }

    create_constant!(kFSEventStreamEventFlagNone);
    create_constant!(kFSEventStreamEventFlagMustScanSubDirs);
    create_constant!(kFSEventStreamEventFlagUserDropped);
    create_constant!(kFSEventStreamEventFlagKernelDropped);
    create_constant!(kFSEventStreamEventFlagEventIdsWrapped);
    create_constant!(kFSEventStreamEventFlagHistoryDone);
    create_constant!(kFSEventStreamEventFlagRootChanged);
    create_constant!(kFSEventStreamEventFlagMount);
    create_constant!(kFSEventStreamEventFlagUnmount);
    create_constant!(kFSEventStreamEventFlagItemCreated);
    create_constant!(kFSEventStreamEventFlagItemRemoved);
    create_constant!(kFSEventStreamEventFlagItemInodeMetaMod);
    create_constant!(kFSEventStreamEventFlagItemRenamed);
    create_constant!(kFSEventStreamEventFlagItemModified);
    create_constant!(kFSEventStreamEventFlagItemFinderInfoMod);
    create_constant!(kFSEventStreamEventFlagItemChangeOwner);
    create_constant!(kFSEventStreamEventFlagItemXattrMod);
    create_constant!(kFSEventStreamEventFlagItemIsFile);
    create_constant!(kFSEventStreamEventFlagItemIsDir);
    create_constant!(kFSEventStreamEventFlagItemIsSymlink);
    create_constant!(kFSEventStreamEventFlagItemIsHardlink);
    create_constant!(kFSEventStreamEventFlagItemIsLastHardlink);
    create_constant!(kFSEventStreamEventFlagOwnEvent);
    create_constant!(kFSEventStreamEventFlagItemCloned);
    Ok(constants)
}

#[module_exports]
fn init(mut exports: JsObject, env: Env) -> Result<()> {
    exports.set_named_property("globals", fse_environment_create())?;
    exports.set_named_property("flags", fse_flags(env))?;
    exports.set_named_property("constants", fse_constants(env))?;
    exports.create_named_method("start", fse_start)?;
    exports.create_named_method("start", fse_stop)?;

    Ok(())
}
