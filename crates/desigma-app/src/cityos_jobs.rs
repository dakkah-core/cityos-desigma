//! One heavy CityOS operation per desktop process. Interactive work precedes refresh.
use std::sync::{Mutex,atomic::{AtomicBool,AtomicUsize,Ordering}};
use std::time::Duration;
static HEAVY: Mutex<()> = Mutex::new(());
static INTERACTIVE: AtomicUsize = AtomicUsize::new(0);
struct Waiting(bool);
impl Drop for Waiting {fn drop(&mut self){if self.0 {INTERACTIVE.fetch_sub(1,Ordering::AcqRel);}}}
pub fn run<T>(interactive:bool,cancel:Option<&AtomicBool>,operation:impl FnOnce()->Result<T,String>)->Result<T,String>{
    if interactive {INTERACTIVE.fetch_add(1,Ordering::AcqRel);}
    let waiting=Waiting(interactive);
    loop {
        if cancel.is_some_and(|c|c.load(Ordering::Acquire)){return Err("CityOS background work cancelled; previous generation retained".into());}
        if interactive || INTERACTIVE.load(Ordering::Acquire)==0 {
            if let Ok(lease)=HEAVY.try_lock(){drop(waiting);let result=operation();drop(lease);return result;}
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn cancelled_jobs_never_execute(){let cancel=AtomicBool::new(true);let mut executed=false;
        assert!(run(false,Some(&cancel),||{executed=true;Ok(())}).is_err());assert!(!executed);}
    #[test] fn failure_releases_the_budget(){assert!(run(true,None,||Err::<(),_>("failure".into())).is_err());assert_eq!(run(true,None,||Ok(7)).unwrap(),7);}
}
