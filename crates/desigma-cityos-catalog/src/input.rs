//! Bounded, correlated pipe writes; a stalled helper cannot block cancellation.
use std::io::Write;
use std::sync::{mpsc::{self, Receiver, SyncSender},atomic::{AtomicBool,Ordering}};
use std::time::{Duration,Instant};

pub(super) struct Input {
    frames: SyncSender<(String,Vec<u8>)>,
    completed: Receiver<(String,Result<(),String>)>,
}
impl Input {
    pub fn new(mut pipe:impl Write+Send+'static)->Self {
        let (frames,requests)=mpsc::sync_channel::<(String,Vec<u8>)>(1);
        let (responses,completed)=mpsc::sync_channel(1);
        std::thread::spawn(move||{
            while let Ok((id,bytes))=requests.recv() {
                let result=pipe.write_all(&bytes).and_then(|_|pipe.write_all(b"\n")).and_then(|_|pipe.flush()).map_err(|e|e.to_string());
                let failed=result.is_err();
                if responses.send((id,result)).is_err() || failed {break;}
            }
        });
        Self {frames,completed}
    }
    pub fn send(&self,id:&str,bytes:Vec<u8>)->Result<(),String> {
        self.frames.try_send((id.into(),bytes)).map_err(|_|"Reader input queue unavailable; previous generation retained".into())
    }
    pub fn written(&self,id:&str,deadline:Instant,cancelled:&AtomicBool)->Result<(),String> {
        let mut cancel_deadline=None;
        loop {
            let now=Instant::now();
            if cancelled.load(Ordering::Acquire) {cancel_deadline.get_or_insert(now+Duration::from_millis(50));}
            // A tiny writable frame may already be in flight. Let it finish so
            // Client can cancel through the protocol and retain a healthy reader.
            // A blocked pipe gets only this bounded handoff, then termination.
            if cancel_deadline.is_some_and(|until|now>=until){return Err("Catalog request cancelled; previous generation retained".into());}
            if now>=deadline{return Err("Reader input timed out; previous generation retained".into());}
            let until=cancel_deadline.map_or(deadline,|until|until.min(deadline));
            match self.completed.recv_timeout(Duration::from_millis(50).min(until.saturating_duration_since(now))) {
                Ok((completed,result))=>{result?;if completed==id{return Ok(());}},
                Err(mpsc::RecvTimeoutError::Timeout)=>{},
                Err(_)=>return Err("Reader input stopped; previous generation retained".into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stopped_pipe_writes_obey_cancellation_deadlines_and_correlate_prior_cancel_receipts() {
        struct Blocked {entered: mpsc::Sender<()>,release: Receiver<()>}
        impl Write for Blocked {
            fn write(&mut self,bytes:&[u8])->std::io::Result<usize>{
                if bytes!=b"\n" {self.entered.send(()).unwrap();self.release.recv().unwrap();}Ok(bytes.len())
            }
            fn flush(&mut self)->std::io::Result<()>{Ok(())}
        }
        for cancelled in [false,true] {
            let (entered,observed)=mpsc::channel();let (release,blocked)=mpsc::channel();
            let input=Input::new(Blocked {entered,release:blocked});
            input.send("request",vec![b'x';1024*1024]).unwrap();observed.recv_timeout(Duration::from_secs(1)).unwrap();
            let flag=AtomicBool::new(cancelled);let start=Instant::now();
            assert!(input.written("request",start+Duration::from_millis(100),&flag).is_err());
            assert!(start.elapsed()<Duration::from_millis(500));release.send(()).unwrap();
            flag.store(false,Ordering::Release);input.written("request",Instant::now()+Duration::from_secs(1),&flag).unwrap();
        }
        let input=Input::new(Vec::<u8>::new());let flag=AtomicBool::new(false);
        input.send("old-cancel",b"{}".to_vec()).unwrap();
        // Wait only in the fixture to ensure the previous ack occupies its one slot.
        let deadline=Instant::now()+Duration::from_secs(1);
        loop {match input.frames.try_send(("new-request".into(),b"{}".to_vec())) {Ok(())=>break,Err(mpsc::TrySendError::Full(_)) if Instant::now()<deadline=>std::thread::yield_now(),Err(_)=>panic!("fixture writer did not drain")}}
        input.written("new-request",deadline,&flag).unwrap();
    }
    #[test]
    fn a_healthy_inflight_write_can_complete_before_protocol_cancellation() {
        let input=Input::new(Vec::<u8>::new());
        input.send("scan",b"{}".to_vec()).unwrap();
        input.written("scan",Instant::now()+Duration::from_secs(1),&AtomicBool::new(true)).unwrap();
    }
}
