use crate::services::{ServiceAction, ServiceManager};
use crate::store::{Run, Snapshot, Store, Task, next_time};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local};
use std::{
    collections::HashMap,
    io::Read,
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver, Sender, SyncSender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const MAX_OUTPUT: usize = 2 * 1024 * 1024;

pub enum Action {
    Save(Task),
    Toggle(i64, bool),
    Delete(i64),
    Run(i64),
    Stop(i64),
    Logs(bool, Option<i64>, i64),
    Output(bool, i64),
    ClearLogs(bool, Option<i64>),
    Service(ServiceAction),
    Shutdown,
}

pub enum Event {
    Saved,
    ServiceSaved,
    Notice(String),
    Error(String),
    Halted(String),
    Logs(bool, Option<i64>, i64, Vec<Run>),
    Output(bool, i64, String),
    LogsCleared(bool, Option<i64>),
}

pub(crate) enum PipeEvent {
    Data(i64, String),
    Closed(i64),
}

pub(crate) struct Running {
    pub(crate) child: Child,
    pub(crate) run_id: i64,
    pub(crate) closed: usize,
    pub(crate) exit: Option<ExitStatus>,
    stopped: bool,
    bytes: usize,
    pub(crate) pending: String,
    #[cfg(windows)]
    job: Option<Job>,
}

impl Running {
    pub(crate) fn kill_tree(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        #[cfg(windows)]
        {
            self.job.take();
        }
        let _ = self.child.kill();
    }

    pub(crate) fn request_stop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGTERM);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Best effort graceful termination; the service manager enforces a deadline.
            let _ = Command::new("taskkill.exe")
                .args(["/PID", &self.child.id().to_string(), "/T"])
                .creation_flags(0x08000000)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map(|mut child| {
                    thread::spawn(move || {
                        let _ = child.wait();
                    })
                });
        }
    }
}

pub struct Engine {
    pub tx: Sender<Action>,
    pub rx: Receiver<Event>,
    pub snapshots: Receiver<Snapshot>,
    worker: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn start(data_dir: PathBuf, default_dir: PathBuf) -> Result<Self> {
        let store = Store::open(&data_dir.join("crondesk.sqlite"))?;
        let (tx, actions) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let (snapshots_tx, snapshots) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let mut worker = Worker {
                store,
                events,
                snapshots: snapshots_tx,
                running: HashMap::new(),
                schedules: HashMap::new(),
                data_dir,
                default_dir,
                services: ServiceManager::new(),
            };
            if let Err(e) = worker.run(actions) {
                let _ = worker
                    .events
                    .send(Event::Halted(format!("调度器已停止：{e:#}")));
            }
            worker.shutdown();
        });
        Ok(Self {
            tx,
            rx,
            snapshots,
            worker: Some(worker),
        })
    }

    pub fn send(&self, action: Action) {
        let _ = self.tx.send(action);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(Action::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Worker {
    store: Store,
    events: Sender<Event>,
    snapshots: SyncSender<Snapshot>,
    running: HashMap<i64, Running>,
    schedules: HashMap<i64, DateTime<Local>>,
    data_dir: PathBuf,
    default_dir: PathBuf,
    services: ServiceManager,
}

impl Worker {
    fn refresh_schedules(&mut self) -> Result<()> {
        self.schedules.clear();
        for task in self.store.tasks()?.iter().filter(|t| t.enabled) {
            self.schedules
                .insert(task.id, next_time(&task.cron, Local::now())?);
        }
        Ok(())
    }

    fn snapshot(&self) -> Result<()> {
        let mut snapshot = self.store.snapshot()?;
        snapshot.running = self.running.len();
        snapshot.services = self.services.snapshot(&self.store)?;
        for task in &mut snapshot.tasks {
            task.running = self.running.contains_key(&task.id);
            task.next = self
                .schedules
                .get(&task.id)
                .map(|t| t.format("%m-%d %H:%M:%S").to_string())
                .unwrap_or_default();
        }
        let _ = self.snapshots.try_send(snapshot);
        Ok(())
    }

    fn run(&mut self, actions: Receiver<Action>) -> Result<()> {
        self.refresh_schedules()?;
        self.services
            .autostart(&self.store, &self.data_dir, &self.default_dir)?;
        self.snapshot()?;
        let (pipes_tx, pipes_rx) = mpsc::sync_channel(64);
        let mut published = Instant::now();
        loop {
            match actions.recv_timeout(Duration::from_millis(100)) {
                Ok(Action::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                Ok(action) => {
                    if let Err(e) = self.action(action, &pipes_tx) {
                        let _ = self.events.send(Event::Error(format!("{e:#}")));
                    }
                    self.snapshot()?;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => (),
            }
            for _ in 0..128 {
                let Ok(event) = pipes_rx.try_recv() else {
                    break;
                };
                match event {
                    PipeEvent::Data(id, text) => {
                        if let Some(run) = self.running.values_mut().find(|r| r.run_id == id)
                            && run.bytes < MAX_OUTPUT
                        {
                            let mut end = text.len().min(MAX_OUTPUT - run.bytes);
                            while !text.is_char_boundary(end) {
                                end -= 1;
                            }
                            run.pending.push_str(&text[..end]);
                            run.bytes += end;
                            if end < text.len() || run.bytes == MAX_OUTPUT {
                                run.pending
                                    .push_str("\n[系统] 日志已达到 2 MiB，后续输出不再保存。\n");
                                run.bytes = MAX_OUTPUT;
                            }
                        }
                    }
                    PipeEvent::Closed(id) => {
                        if let Some(run) = self.running.values_mut().find(|r| r.run_id == id) {
                            run.closed += 1;
                        }
                    }
                }
            }
            let mut finished = Vec::new();
            for (&id, run) in &mut self.running {
                if !run.pending.is_empty() {
                    self.store.append(run.run_id, &run.pending)?;
                    run.pending.clear();
                }
                if run.exit.is_none()
                    && let Some(exit) = run.child.try_wait()?
                {
                    run.exit = Some(exit);
                    // Background descendants must not outlive a completed task.
                    run.kill_tree();
                }
                if run.closed == 2
                    && let Some(exit) = run.exit
                {
                    let status = if run.stopped {
                        "interrupted"
                    } else if exit.success() {
                        "success"
                    } else {
                        "failed"
                    };
                    self.store.finish(run.run_id, status, exit.code())?;
                    finished.push(id);
                }
            }
            for id in finished {
                if let Some(run) = self.running.remove(&id) {
                    self.remove_script(run.run_id);
                }
            }
            let now = Local::now();
            self.services
                .tick(&self.store, &self.data_dir, &self.default_dir)?;
            let tasks = self.store.tasks()?;
            for task in tasks.iter().filter(|t| t.enabled) {
                if let Some(&next) = self.schedules.get(&task.id)
                    && now >= next
                {
                    self.schedules.insert(task.id, next_time(&task.cron, now)?);
                    // Do not replay missed occurrences after sleep or a forward clock jump.
                    if (now - next).num_seconds() < 3 {
                        if self.running.contains_key(&task.id) {
                            let run = self.store.begin_run(task, "scheduled", &self.cwd(task))?;
                            self.store
                                .append(run, "[系统] 上次执行尚未结束，本次触发已跳过。\n")?;
                            self.store.finish(run, "skipped", None)?;
                        } else if let Err(e) = self.launch(task, "scheduled", &pipes_tx) {
                            let _ = self
                                .events
                                .send(Event::Error(format!("{}：{e:#}", task.name)));
                        }
                    }
                }
            }
            if published.elapsed() >= Duration::from_secs(1) {
                self.snapshot()?;
                published = Instant::now();
            }
        }
    }

    fn action(&mut self, action: Action, pipes: &SyncSender<PipeEvent>) -> Result<()> {
        match action {
            Action::Save(task) => {
                if self.running.contains_key(&task.id) {
                    bail!("请先停止正在执行的任务再编辑");
                }
                self.store.save(&task)?;
                let id = if task.id == 0 {
                    self.store.0.last_insert_rowid()
                } else {
                    task.id
                };
                if task.enabled {
                    self.schedules
                        .insert(id, next_time(&task.cron, Local::now())?);
                } else {
                    self.schedules.remove(&id);
                }
                let _ = self.events.send(Event::Saved);
            }
            Action::Toggle(id, enabled) => {
                self.store.0.execute(
                    "UPDATE tasks SET enabled=?1 WHERE id=?2",
                    rusqlite::params![enabled, id],
                )?;
                if enabled {
                    if let Some(task) = self.store.tasks()?.iter().find(|t| t.id == id) {
                        self.schedules
                            .insert(id, next_time(&task.cron, Local::now())?);
                    }
                } else {
                    self.schedules.remove(&id);
                }
            }
            Action::Delete(id) => {
                if self.running.contains_key(&id) {
                    bail!("请先停止正在执行的任务再删除");
                }
                let tx = self.store.0.unchecked_transaction()?;
                tx.execute("DELETE FROM runs WHERE task_id=?1", [id])?;
                tx.execute("DELETE FROM tasks WHERE id=?1", [id])?;
                tx.commit()?;
                self.schedules.remove(&id);
                let _ = self.events.send(Event::LogsCleared(false, Some(id)));
                let _ = self
                    .events
                    .send(Event::Notice("任务及相关日志已删除".into()));
            }
            Action::Run(id) => {
                let task = self
                    .store
                    .tasks()?
                    .into_iter()
                    .find(|t| t.id == id)
                    .context("任务已不存在")?;
                self.launch(&task, "manual", pipes)?;
            }
            Action::Stop(id) => {
                if let Some(run) = self.running.get_mut(&id) {
                    run.stopped = true;
                    self.store.append(run.run_id, "\n[系统] 用户停止执行。\n")?;
                    run.kill_tree();
                }
            }
            Action::Logs(service, id, before) => {
                let runs = if service {
                    self.store.service_logs(id, before)?
                } else {
                    self.store.logs(id, before)?
                };
                let _ = self.events.send(Event::Logs(service, id, before, runs));
            }
            Action::Output(service, id) => {
                let output = if service {
                    self.store.service_output(id)?
                } else {
                    self.store.output(id)?
                };
                let _ = self.events.send(Event::Output(service, id, output));
            }
            Action::ClearLogs(service, owner) => {
                let busy = if service {
                    self.services.has_active(owner)
                } else {
                    self.running
                        .keys()
                        .any(|id| owner.is_none_or(|owner| owner == *id))
                };
                if busy {
                    bail!("请先停止清理范围内的任务或服务，再清空日志");
                }
                self.store.clear_logs(service, owner)?;
                let _ = self.events.send(Event::LogsCleared(service, owner));
                let _ = self.events.send(Event::Notice("日志已清空".into()));
            }
            Action::Service(action) => {
                let deleted = if let ServiceAction::Delete(id) = &action {
                    Some(*id)
                } else {
                    None
                };
                if self
                    .services
                    .action(action, &self.store, &self.data_dir, &self.default_dir)?
                {
                    let _ = self.events.send(Event::ServiceSaved);
                }
                if let Some(id) = deleted {
                    let _ = self.events.send(Event::LogsCleared(true, Some(id)));
                    let _ = self
                        .events
                        .send(Event::Notice("服务及相关日志已删除".into()));
                }
            }
            Action::Shutdown => (),
        }
        Ok(())
    }

    fn cwd(&self, task: &Task) -> String {
        if task.cwd.is_empty() {
            self.default_dir.to_string_lossy().into_owned()
        } else {
            task.cwd.clone()
        }
    }

    fn launch(&mut self, task: &Task, trigger: &str, pipes: &SyncSender<PipeEvent>) -> Result<()> {
        if self.running.contains_key(&task.id) {
            bail!("该任务正在执行，请勿重复启动");
        }
        let cwd = self.cwd(task);
        let id = self.store.begin_run(task, trigger, &cwd)?;
        match spawn(task, &cwd, id, &self.data_dir) {
            Ok(mut running) => {
                read_pipe(
                    running.child.stdout.take().unwrap(),
                    id,
                    false,
                    pipes.clone(),
                );
                read_pipe(
                    running.child.stderr.take().unwrap(),
                    id,
                    true,
                    pipes.clone(),
                );
                self.running.insert(task.id, running);
            }
            Err(e) => {
                self.store
                    .append(id, &format!("[系统] 启动失败：{e:#}\n"))?;
                self.store.finish(id, "failed", None)?;
                self.remove_script(id);
            }
        }
        Ok(())
    }

    fn remove_script(&self, id: i64) {
        #[cfg(windows)]
        let _ = std::fs::remove_file(self.data_dir.join(format!("run-{id}.cmd")));
        #[cfg(not(windows))]
        let _ = id;
    }

    fn shutdown(&mut self) {
        self.services.shutdown(&self.store, &self.data_dir);
        for run in self.running.values_mut() {
            run.kill_tree();
            let _ = run.child.wait();
            let _ = self.store.append(run.run_id, &run.pending);
            let _ = self
                .store
                .append(run.run_id, "\n[系统] 应用退出，任务已停止。\n");
            let _ = self.store.finish(run.run_id, "interrupted", None);
        }
        for run in self.running.values() {
            self.remove_script(run.run_id);
        }
        let _ = self
            .store
            .0
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
    }
}

pub(crate) fn read_pipe(
    mut pipe: impl Read + Send + 'static,
    id: i64,
    stderr: bool,
    tx: SyncSender<PipeEvent>,
) {
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        loop {
            let n = match pipe.read(&mut buffer) {
                Ok(n) => n,
                Err(e) => {
                    let _ = tx.send(PipeEvent::Data(id, format!("\n[系统] 读取输出失败：{e}\n")));
                    break;
                }
            };
            let mut text = String::with_capacity(n * 3 + 4);
            let _ = decoder.decode_to_string(&buffer[..n], &mut text, n == 0);
            if !text.is_empty() {
                if stderr {
                    text.insert_str(0, "[stderr] ");
                }
                if tx.send(PipeEvent::Data(id, text)).is_err() {
                    return;
                }
            }
            if n == 0 {
                break;
            }
        }
        let _ = tx.send(PipeEvent::Closed(id));
    });
}

pub(crate) fn spawn(
    task: &Task,
    cwd: &str,
    id: i64,
    data_dir: &std::path::Path,
) -> Result<Running> {
    #[cfg(unix)]
    let mut command = {
        use std::os::unix::process::CommandExt;
        let _ = data_dir;
        let mut c = Command::new("/bin/sh");
        c.args(["-c", &task.command]).process_group(0);
        c
    };
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let script = data_dir.join(format!("run-{id}.cmd"));
        let body = task.command.replace("\r\n", "\n").replace('\n', "\r\n");
        std::fs::write(
            &script,
            format!("@echo off\r\nchcp 65001 >nul\r\n{body}\r\n"),
        )?;
        let mut c = Command::new("cmd.exe");
        c.args(["/D", "/S", "/C"])
            .raw_arg(format!("\"\"{}\"\"", script.display()));
        c.creation_flags(0x08000000); // CREATE_NO_WINDOW
        c
    };
    command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUNBUFFERED", "1");
    let child = command
        .spawn()
        .context("无法启动命令，请检查脚本、执行目录和环境变量 PATH")?;
    #[cfg(windows)]
    let (child, job) = {
        let mut child = child;
        match Job::attach(&child) {
            Ok(job) => (child, Some(job)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        }
    };
    Ok(Running {
        child,
        run_id: id,
        closed: 0,
        exit: None,
        stopped: false,
        bytes: 0,
        pending: String::new(),
        #[cfg(windows)]
        job,
    })
}

#[cfg(windows)]
struct Job(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Job {
    fn attach(child: &Child) -> Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(std::io::Error::last_os_error().into());
            }
            let job = Self(handle);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            ) == 0
                || AssignProcessToJobObject(handle, child.as_raw_handle()) == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(job)
        }
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
