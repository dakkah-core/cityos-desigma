//! Temporary catalog views live outside App.doc and never enter editor save/history.
use desigma_cityos_catalog::{Client, Entry, Filters, Page, ReaderConfig};
use egui::{Context, Ui};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

enum Job {
    Scan(u64, ReaderConfig, String),
    Query(u64, Filters, usize),
    Materialize(u64, String),
}
enum Reply {
    Scan(u64, Result<Value,String>),
    Query(u64, Result<Page,String>),
    Materialize(u64, Result<Value,String>),
}
pub struct Browser {
    pub open: bool,
    repository: String,
    reader: String,
    node: String,
    cache: Option<PathBuf>,
    canonical_ref: Option<String>,
    mode: String,
    filters: Filters,
    jobs: Sender<Job>,
    replies: Receiver<Reply>,
    epoch: u64,
    busy: bool,
    summary: Option<Value>,
    entries: Vec<Entry>,
    total: usize,
    next: Option<usize>,
    selected: Option<Entry>,
    specimen: Option<Value>,
    error: Option<String>,
    last_refresh: Instant,
    filter_changed: Option<Instant>,
}

impl Browser {
    pub fn new(repository: Option<PathBuf>, reader: Option<PathBuf>, node: Option<PathBuf>, cache: Option<PathBuf>, canonical_ref: Option<String>) -> Self {
        let (jobs, requests)=mpsc::channel(); let (responses,replies)=mpsc::channel();
        std::thread::spawn(move || {
            let mut client: Option<Client>=None;
            while let Ok(job)=requests.recv() {
                let reply=match job {
                    Job::Scan(epoch,config,mode)=>{
                        // Each scan gets a new session; stale responses cannot escape the old child.
                        client=None;
                        let result=Client::connect(&config).and_then(|mut c|{let result=c.scan(&config,&mode);client=Some(c);result});
                        Reply::Scan(epoch,result)
                    }
                    Job::Query(epoch,filters,offset)=>Reply::Query(epoch,client.as_mut().ok_or_else(||"Refresh the repository first".to_string()).and_then(|c|c.query(&filters,offset))),
                    Job::Materialize(epoch,id)=>Reply::Materialize(epoch,client.as_mut().ok_or_else(||"Refresh the repository first".to_string()).and_then(|c|c.materialize(&id))),
                };
                if responses.send(reply).is_err(){break;}
            }
        });
        let open=repository.is_some();
        let reader=reader.or_else(||std::env::current_exe().ok().map(|p|p.with_file_name("cityos-reader").join("reader.mjs")));
        let mut browser=Self {open,repository:repository.map(|p|p.to_string_lossy().into_owned()).unwrap_or_default(),reader:reader.map(|p|p.to_string_lossy().into_owned()).unwrap_or_default(),
            node:node.map(|p|p.to_string_lossy().into_owned()).unwrap_or_else(||"node".into()),cache,canonical_ref,mode:"committed".into(),filters:Filters::default(),jobs,replies,epoch:0,busy:false,
            summary:None,entries:vec![],total:0,next:None,selected:None,specimen:None,error:None,last_refresh:Instant::now(),filter_changed:None};
        if open {browser.refresh();} browser
    }
    fn refresh(&mut self){
        if self.busy{return;}
        self.epoch+=1;self.busy=true;self.error=None;self.last_refresh=Instant::now();
        let config=ReaderConfig {node:self.node.clone().into(),reader:self.reader.clone().into(),repository:self.repository.clone().into(),cache:self.cache.clone(),canonical_ref:self.canonical_ref.clone()};
        let _=self.jobs.send(Job::Scan(self.epoch,config,self.mode.clone()));
    }
    fn query(&mut self,offset:usize){self.epoch+=1;self.busy=true;let _=self.jobs.send(Job::Query(self.epoch,self.filters.clone(),offset));}
    fn poll(&mut self){while let Ok(reply)=self.replies.try_recv(){match reply {
        Reply::Scan(epoch,result) if epoch==self.epoch=>{self.busy=false;match result {Ok(summary)=>{self.summary=Some(summary);self.query(0);},Err(e)=>self.error=Some(e)}}
        Reply::Query(epoch,result) if epoch==self.epoch=>{self.busy=false;match result {Ok(page)=>{self.entries=page.entries;self.total=page.total;self.next=page.next_offset;},Err(e)=>self.error=Some(e)}}
        Reply::Materialize(epoch,result) if epoch==self.epoch=>{self.busy=false;match result {Ok(value)=>self.specimen=Some(value),Err(e)=>self.error=Some(e)}}
        _=>{}
    }}}
    pub fn show(&mut self,ui:&mut Ui,ctx:&Context){
        self.poll();ctx.request_repaint_after(Duration::from_millis(100));
        if !self.busy&&self.summary.is_some()&&self.last_refresh.elapsed()>=Duration::from_secs(60){self.refresh();}
        if !self.busy&&self.filter_changed.is_some_and(|t|t.elapsed()>=Duration::from_millis(750)){self.filter_changed=None;self.query(0);}
        ui.horizontal(|ui|{if ui.button("← Back to files").clicked(){self.open=false;} ui.heading("CityOS Catalog");ui.label("Read-only · data-only views");});
        ui.separator();
        ui.horizontal(|ui|{ui.label("Repository");ui.text_edit_singleline(&mut self.repository);if ui.button("Choose repository…").clicked(){if let Some(p)=rfd::FileDialog::new().pick_folder(){self.repository=p.to_string_lossy().into_owned();}}});
        ui.horizontal(|ui|{ui.label("Packaged reader");ui.text_edit_singleline(&mut self.reader);if ui.button("Choose reader…").clicked(){if let Some(p)=rfd::FileDialog::new().add_filter("CityOS reader",&["mjs"]).pick_file(){self.reader=p.to_string_lossy().into_owned();}}});
        ui.horizontal(|ui|{
            egui::ComboBox::from_id_salt("cityos-source").selected_text(if self.mode=="local"{"Local preview"}else{"Committed CityOS"}).show_ui(ui,|ui|{ui.selectable_value(&mut self.mode,"committed".into(),"Committed CityOS");ui.selectable_value(&mut self.mode,"local".into(),"Local preview");});
            if ui.add_enabled(!self.busy,egui::Button::new("Refresh")).clicked(){self.refresh();}
            if self.busy{ui.spinner();ui.label("Capturing source…");}
            if let Some(summary)=&self.summary{let sha=summary["snapshot"]["canonicalCommit"].as_str().unwrap_or("");ui.label(format!("{} · {} entries",sha.get(..12).unwrap_or(sha),summary["entryCount"]));
                ui.label(if summary["accountingComplete"]==true{"Source accounting complete"}else{"Source accounting incomplete"});}
        });
        if let Some(error)=&self.error{ui.colored_label(egui::Color32::from_rgb(190,85,70),format!("Refresh/view error: {error}. Previous view retained."));}
        let mut changed=false;
        ui.horizontal_wrapped(|ui|{
            changed|=ui.add(egui::TextEdit::singleline(&mut self.filters.search).hint_text("Search components, shells, tokens and sources").desired_width(300.0)).changed();
            egui::ComboBox::from_id_salt("cityos-level").selected_text(if self.filters.level.is_empty(){"All L0–L9"}else{&self.filters.level}).show_ui(ui,|ui|{changed|=ui.selectable_value(&mut self.filters.level,String::new(),"All levels").changed();if let Some(levels)=self.summary.as_ref().and_then(|s|s["levels"].as_array()){for level in levels {let id=level["id"].as_str().unwrap_or("");changed|=ui.selectable_value(&mut self.filters.level,id.into(),format!("{id} {}",level["name"].as_str().unwrap_or(""))).changed();}}});
            for (label,value) in [("Product",&mut self.filters.product),("Target",&mut self.filters.target),("Owner",&mut self.filters.owner),("Package",&mut self.filters.package),("Locale",&mut self.filters.locale),("Status",&mut self.filters.status)]{
                ui.label(label);changed|=ui.add(egui::TextEdit::singleline(value).desired_width(110.0)).changed();}
        });if changed{self.filter_changed=Some(Instant::now());}
        ui.separator();
        let available=ui.available_size();
        ui.columns(2,|columns|{
            columns[0].label(format!("{} matching entries",self.total));
            egui::ScrollArea::vertical().id_salt("cityos-entries").max_height((available.y-80.0).max(120.0)).show(&mut columns[0],|ui|{
                let mut selection=None;
                for entry in &self.entries{let label=format!("{}  {}  · {}",entry.level.as_deref().unwrap_or("?"),entry.label,entry.product.as_deref().unwrap_or(&entry.family));
                    if ui.selectable_label(self.selected.as_ref().is_some_and(|s|s.id==entry.id),label).clicked(){selection=Some(entry.clone());}}
                if let Some(entry)=selection{self.epoch+=1;self.busy=true;self.specimen=None;let _=self.jobs.send(Job::Materialize(self.epoch,entry.id.clone()));self.selected=Some(entry);}
            });
            if let Some(offset)=self.next{if columns[0].add_enabled(!self.busy,egui::Button::new("Next 256 entries →")).clicked(){self.query(offset);}}
            if columns[0].add_enabled(!self.busy,egui::Button::new("First page")).clicked(){self.query(0);}
            egui::ScrollArea::vertical().id_salt("cityos-inspector").max_height((available.y-40.0).max(120.0)).show(&mut columns[1],|ui|{
                if let Some(entry)=&self.selected{
                    ui.heading(&entry.label);ui.monospace(&entry.semantic_id);ui.label(format!("{} · {} · {}",entry.kind,entry.owner,entry.classification_status));
                    ui.monospace(format!("{}#{}",entry.source_path,entry.source_pointer));ui.label(format!("Observed generation: {}",entry.generation_status));
                    ui.monospace(&entry.source_digest);ui.label(format!("Targets: {}",entry.targets.join(", ")));ui.label("Persistent drafts, edits and target rendering: unqualified");
                    ui.collapsing("Dependencies",|ui|{for dep in &entry.dependencies{ui.monospace(dep);}});
                    for finding in &entry.diagnostics{ui.label(finding["message"].as_str().unwrap_or("Source diagnostic"));}
                    if let Some(specimen)=&self.specimen{ui.separator();ui.strong("Temporary data-only view");let mut text=serde_json::to_string_pretty(&specimen["data"]).unwrap_or_default();ui.add(egui::TextEdit::multiline(&mut text).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).interactive(false));}
                }else{ui.label("Select a source item to inspect its captured contract.");}
                if let Some(summary)=&self.summary{ui.collapsing("Source accounting and capability evidence",|ui|{ui.monospace(serde_json::to_string_pretty(&serde_json::json!({"snapshot":summary["snapshot"],"census":summary["census"],"capabilities":summary["capabilities"],"diagnostics":summary["diagnostics"],"native":summary["native"]})).unwrap_or_default());});}
            });
        });
    }
}
