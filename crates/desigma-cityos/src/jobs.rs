//! One heavy CityOS operation per desktop process. Interactive work precedes refresh.
use std::sync::{Mutex,atomic::{AtomicBool,AtomicUsize,Ordering}};
use std::time::Duration;
use std::time::Instant;
static HEAVY: Mutex<()> = Mutex::new(());
static INTERACTIVE: AtomicUsize = AtomicUsize::new(0);
static BACKGROUND_PAUSED: AtomicBool = AtomicBool::new(false);
static LAST_INTERACTION: Mutex<Option<Instant>> = Mutex::new(None);
/// Job events and bounded polls drive progress; background indexing must not
/// animate a spinner at the display refresh rate.
pub fn progress_marker(ui: &mut egui::Ui) { ui.label("…"); }

#[cfg(test)]
#[test]
fn progress_marker_does_not_schedule_animation_frames() {
    let ctx = egui::Context::default();
    for _ in 0..8 { let _ = ctx.run_ui(Default::default(), progress_marker); }
    let immediate = std::sync::Arc::new(AtomicUsize::new(0));
    let observed = immediate.clone();
    ctx.set_request_repaint_callback(move |info| {
        if info.delay.is_zero() { observed.fetch_add(1, Ordering::SeqCst); }
    });
    for _ in 0..8 { let _ = ctx.run_ui(Default::default(), progress_marker); }
    assert_eq!(immediate.load(Ordering::SeqCst), 0);
}
/// Window state affects speculative/refresh jobs, never saves or user commands.
pub fn set_background_paused(paused: bool) { BACKGROUND_PAUSED.store(paused, Ordering::Release); }
pub fn note_interaction() { *LAST_INTERACTION.lock().unwrap_or_else(|e|e.into_inner())=Some(Instant::now()); }
#[derive(Default)]
struct Pressure {memory_paused:bool,commit_paused:bool,cpu_paused:bool,high_since:Option<Instant>,low_since:Option<Instant>,last_check:Option<Instant>,last_cpu:Option<(u64,u64)>}
impl Pressure {
    fn update_commit(&mut self,bytes:u64)->bool {
        if bytes<1024*1024*1024 {self.commit_paused=true;}else if bytes>2*1024*1024*1024 {self.commit_paused=false;}
        self.commit_paused
    }
    fn update(&mut self,now:Instant,memory:u64,cpu:Option<f64>)->Option<&'static str>{
        if memory<4*1024*1024*1024{self.memory_paused=true;}else if memory>5*1024*1024*1024{self.memory_paused=false;}
        if let Some(cpu)=cpu {if cpu>0.70 {self.low_since=None;let start=*self.high_since.get_or_insert(now);if now.duration_since(start)>=Duration::from_secs(10){self.cpu_paused=true;}}
            else if cpu<0.50 {self.high_since=None;let start=*self.low_since.get_or_insert(now);if now.duration_since(start)>=Duration::from_secs(10){self.cpu_paused=false;}}
            else {self.high_since=None;self.low_since=None;}}
        if self.memory_paused{Some("Background indexing paused: less than 4 GiB available RAM")}else if self.cpu_paused{Some("Background indexing paused: system CPU pressure")}else{None}
    }
}
static PRESSURE:Mutex<Pressure>=Mutex::new(Pressure{memory_paused:false,commit_paused:false,cpu_paused:false,high_since:None,low_since:None,last_check:None,last_cpu:None});
pub fn resource_status()->Option<&'static str>{
    if BACKGROUND_PAUSED.load(Ordering::Acquire) { return Some("Background work paused while the window is minimized or in use"); }
    if LAST_INTERACTION.lock().unwrap_or_else(|e|e.into_inner()).is_some_and(|at|at.elapsed()<Duration::from_millis(750)) {
        return Some("Background work paused while interacting");
    }
    #[cfg(windows)] {
        #[repr(C)] struct Memory {length:u32,load:u32,total:u64,available:u64,total_page:u64,available_page:u64,total_virtual:u64,available_virtual:u64,extended:u64}
        #[repr(C)] #[derive(Default)] struct FileTime {low:u32,high:u32}
        impl FileTime {fn value(&self)->u64 {((self.high as u64)<<32)|self.low as u64}}
        #[link(name="kernel32")] unsafe extern "system" {fn GlobalMemoryStatusEx(memory:*mut Memory)->i32;fn GetSystemTimes(idle:*mut FileTime,kernel:*mut FileTime,user:*mut FileTime)->i32;}
        let mut pressure=PRESSURE.lock().unwrap_or_else(|e|e.into_inner());let now=Instant::now();
        if pressure.last_check.is_none_or(|t|now.duration_since(t)>=Duration::from_millis(500)){
            pressure.last_check=Some(now);let mut memory=Memory{length:std::mem::size_of::<Memory>() as u32,load:0,total:0,available:u64::MAX,total_page:0,available_page:0,total_virtual:0,available_virtual:0,extended:0};
            let(mut idle,mut kernel,mut user)=(FileTime::default(),FileTime::default(),FileTime::default());
            // SAFETY: these correctly sized output structures live throughout both calls.
            unsafe {if GlobalMemoryStatusEx(&mut memory)==0{pressure.memory_paused=true;return Some("Background indexing paused: resource probe unavailable");}
                if pressure.update_commit(memory.available_page){return Some("Background work paused: Windows commit headroom is below 1 GiB");}
                let cpu=if GetSystemTimes(&mut idle,&mut kernel,&mut user)!=0 {let current=(idle.value(),kernel.value().saturating_add(user.value()));
                    let value=pressure.last_cpu.and_then(|last|{let total=current.1.saturating_sub(last.1);(total>0).then(||1.0-current.0.saturating_sub(last.0) as f64/total as f64)});pressure.last_cpu=Some(current);value}else{None};
                return pressure.update(now,memory.available,cpu);}
        }
        if pressure.commit_paused {Some("Background work paused: Windows commit pressure")}else if pressure.memory_paused {Some("Background indexing paused: memory pressure")}else if pressure.cpu_paused{Some("Background indexing paused: CPU pressure")}else{None}
    }
    #[cfg(not(windows))] {None}
}
struct Waiting(bool);
impl Drop for Waiting {fn drop(&mut self){if self.0 {INTERACTIVE.fetch_sub(1,Ordering::AcqRel);}}}
pub fn run<T>(interactive:bool,cancel:Option<&AtomicBool>,operation:impl FnOnce()->Result<T,String>)->Result<T,String>{
    if interactive {INTERACTIVE.fetch_add(1,Ordering::AcqRel);}
    let waiting=Waiting(interactive);
    loop {
        if cancel.is_some_and(|c|c.load(Ordering::Acquire)){return Err("CityOS background work cancelled; previous generation retained".into());}
        if interactive || (INTERACTIVE.load(Ordering::Acquire)==0 && resource_status().is_none()) {
            if let Ok(lease)=HEAVY.try_lock(){drop(waiting);let result=operation();drop(lease);return result;}
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn cancelled_jobs_never_execute(){let cancel=AtomicBool::new(true);let mut executed=false;
        assert!(run(false,Some(&cancel),||{executed=true;Ok(())}).is_err());assert!(!executed);}
    #[test] fn commit_pressure_pauses_even_when_physical_ram_is_available(){let mut pressure=Pressure::default();
        assert!(pressure.update_commit(512*1024*1024));assert!(pressure.update_commit(1536*1024*1024));
        assert!(!pressure.update_commit(3*1024*1024*1024));}
    #[test] fn failure_releases_the_budget(){assert!(run(true,None,||Err::<(),_>("failure".into())).is_err());assert_eq!(run(true,None,||Ok(7)).unwrap(),7);}
    #[test] fn resource_hysteresis_prevents_repeated_pause_resume(){let mut pressure=Pressure::default();let now=Instant::now();
        assert!(pressure.update(now,3*1024*1024*1024,None).is_some());assert!(pressure.update(now,4*1024*1024*1024,None).is_some());assert!(pressure.update(now,6*1024*1024*1024,None).is_none());
        pressure.update(now,6*1024*1024*1024,Some(0.8));assert!(pressure.update(now+Duration::from_secs(11),6*1024*1024*1024,Some(0.8)).is_some());
        assert!(pressure.update(now+Duration::from_secs(12),6*1024*1024*1024,Some(0.1)).is_some());assert!(pressure.update(now+Duration::from_secs(23),6*1024*1024*1024,Some(0.1)).is_none());}
}
