//! macOS delivers `desigma://` links and Finder file opens as Apple events (`GURL`/`GURL` and
//! `aevt`/`odoc`), not as arguments, and winit does not surface them. A small Objective-C object
//! registered with `NSAppleEventManager` receives them instead and queues them in
//! `instance::push`; the UI drains that queue every frame. [`register`] is safe to call early
//! (events that arrive before the window exists wait in the queue) and again once the app has
//! launched, because Cocoa installs its own handlers while launching and replaces ours.

use crate::instance::{self, Open};
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{declare_class, msg_send, msg_send_id, mutability, sel, ClassType, DeclaredClass};
use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager};
use std::cell::RefCell;

/// `kInternetEventClass` and `kAEGetURL` are both 'GURL'.
const GURL: u32 = u32::from_be_bytes(*b"GURL");
/// `kCoreEventClass` 'aevt' and `kAEOpenDocuments` 'odoc'.
const AEVT: u32 = u32::from_be_bytes(*b"aevt");
const ODOC: u32 = u32::from_be_bytes(*b"odoc");
/// `keyDirectObject` '----' and `typeFileURL` 'furl'.
const KEY_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
const TYPE_FILE_URL: u32 = u32::from_be_bytes(*b"furl");

declare_class!(
    struct AppleEventHandler;

    unsafe impl ClassType for AppleEventHandler {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "DesigmaAppleEventHandler";
    }

    impl DeclaredClass for AppleEventHandler {}

    unsafe impl AppleEventHandler {
        #[method(handleURL:withReplyEvent:)]
        fn handle_url(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            if let Some(url) = direct_object(event).and_then(|d| unsafe { d.stringValue() }) {
                instance::push(Open::Link(url.to_string()));
            }
        }

        #[method(handleOpenDocuments:withReplyEvent:)]
        fn handle_open_documents(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            let Some(list) = direct_object(event) else { return };
            // the items of a list are 1-based
            for i in 1..=unsafe { list.numberOfItems() } {
                let item = unsafe { list.descriptorAtIndex(i) };
                if let Some(path) = item.and_then(|d| file_path(&d)) {
                    instance::push(Open::File(path));
                }
            }
        }
    }
);

fn direct_object(event: &NSAppleEventDescriptor) -> Option<Retained<NSAppleEventDescriptor>> {
    unsafe { msg_send_id![event, paramDescriptorForKeyword: KEY_DIRECT_OBJECT] }
}

/// The path of a file item (an alias or file reference in the event), through its file URL.
fn file_path(item: &NSAppleEventDescriptor) -> Option<std::path::PathBuf> {
    let url: Option<Retained<NSAppleEventDescriptor>> = unsafe { msg_send_id![item, coerceToDescriptorType: TYPE_FILE_URL] };
    let url = url?;
    let data = unsafe { url.data() };
    let text = std::str::from_utf8(data.bytes()).ok()?.trim_end_matches('\0');
    url::Url::parse(text).ok()?.to_file_path().ok()
}

thread_local! {
    // The manager does not retain its handler, so keep it for the life of the main thread.
    static HANDLER: RefCell<Option<Retained<AppleEventHandler>>> = const { RefCell::new(None) };
}

/// Registers the handlers for links and Finder opens. Main thread only (both callers are).
pub fn register() {
    HANDLER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let handler = slot.get_or_insert_with(|| {
            let this = AppleEventHandler::alloc().set_ivars(());
            unsafe { msg_send_id![super(this), init] }
        });
        unsafe {
            let manager = NSAppleEventManager::sharedAppleEventManager();
            let _: () = msg_send![&*manager, setEventHandler: &**handler, andSelector: sel!(handleURL:withReplyEvent:), forEventClass: GURL, andEventID: GURL];
            let _: () =
                msg_send![&*manager, setEventHandler: &**handler, andSelector: sel!(handleOpenDocuments:withReplyEvent:), forEventClass: AEVT, andEventID: ODOC];
        }
    });
}
