use crate::{
    engine::{PipeEvent, Running, read_pipe, spawn},
    store::{Run, Store, Task},
};
use anyhow::{Context, Result, bail};
use chrono::Local;
use rusqlite::{Connection, params};
use std::{
    collections::HashMap,
    path::Path,
    sync::mpsc::{self, Receiver, SyncSender},
    time::{Duration, Instant},
};

const RETAIN_BYTES: i64 = 100 * 1024 * 1024;
const VIEW_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Service {
    pub id: i64,
    pub name: String,
    pub command: String,
    pub cwd: String,
    pub powershell: bool,
    pub load_profile: bool,
    pub autostart: bool,
    pub restart_on_failure: bool,
    pub restart_delay: u32,
    pub max_restarts: u32,
    pub status: String,
    pub pid: Option<u32>,
    pub uptime: u64,
    pub restarts: u32,
    pub started: String,
    pub exit_code: Option<i32>,
}

impl Default for Service {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            command: String::new(),
            cwd: String::new(),
            powershell: false,
            load_profile: true,
            autostart: false,
            restart_on_failure: false,
            restart_delay: 5,
            max_restarts: 3,
            status: "stopped".into(),
            pid: None,
            uptime: 0,
            restarts: 0,
            started: String::new(),
            exit_code: None,
        }
    }
}

pub fn validate(service: &Service) -> Result<()> {
    if service.name.trim().is_empty() || service.name.chars().count() > 100 {
        bail!("服务名称不能为空，且不能超过 100 个字符");
    }
    if service.command.trim().is_empty() || service.command.contains('\0') {
        bail!("请填写有效的启动命令");
    }
    if !service.cwd.is_empty()
        && (!Path::new(&service.cwd).is_absolute() || !Path::new(&service.cwd).is_dir())
    {
        bail!("服务执行目录必须是已存在的绝对路径");
    }
    if !(1..=3600).contains(&service.restart_delay) || !(1..=100).contains(&service.max_restarts) {
        bail!("重试间隔需为 1–3600 秒，次数上限需为 1–100 次");
    }
    Ok(())
}

// Additive migration: never recreate, truncate or replace the existing task tables.
pub fn migrate(db: &Connection, path: &Path) -> Result<()> {
    let exists = |table: &str| -> rusqlite::Result<bool> {
        db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [table],
            |r| r.get(0),
        )
    };
    if !exists("services")? {
        if exists("tasks")? {
            let backup = path.with_file_name("crondesk.before-0.2.0.sqlite");
            if !backup.exists() {
                // SQLite creates a consistent snapshot, including committed WAL data.
                db.execute("VACUUM INTO ?1", [backup.to_string_lossy().as_ref()])
                    .context("升级前备份数据库失败，已停止升级以保留原数据")?;
            }
        }
        let tx = db.unchecked_transaction()?;
        tx.execute_batch(
            "CREATE TABLE services (
                id INTEGER PRIMARY KEY, name TEXT NOT NULL, command TEXT NOT NULL, cwd TEXT NOT NULL,
                powershell INTEGER NOT NULL, load_profile INTEGER NOT NULL, autostart INTEGER NOT NULL,
                restart_on_failure INTEGER NOT NULL, restart_delay INTEGER NOT NULL, max_restarts INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'stopped'
             );
             CREATE TABLE service_runs (
                id INTEGER PRIMARY KEY, service_id INTEGER REFERENCES services(id) ON DELETE SET NULL,
                name TEXT NOT NULL, command TEXT NOT NULL, cwd TEXT NOT NULL,
                trigger TEXT NOT NULL, started TEXT NOT NULL, finished TEXT, status TEXT NOT NULL,
                exit_code INTEGER
             );
             CREATE TABLE service_output (
                id INTEGER PRIMARY KEY, run_id INTEGER NOT NULL REFERENCES service_runs(id) ON DELETE CASCADE,
                text TEXT NOT NULL, bytes INTEGER NOT NULL
             );
             CREATE INDEX service_runs_service ON service_runs(service_id,id DESC);
             CREATE INDEX service_output_run ON service_output(run_id,id);
             PRAGMA user_version=2;"
        )?;
        tx.commit()?;
    }
    let tx = db.unchecked_transaction()?;
    let interrupted = "\n[系统] 上次应用异常退出，服务运行结果未知。\n";
    tx.execute("INSERT INTO service_output(run_id,text,bytes) SELECT id,?1,?2 FROM service_runs WHERE status='running'",
        params![interrupted, interrupted.len() as i64])?;
    tx.execute(
        "UPDATE service_runs SET status='interrupted',finished=?1 WHERE status='running'",
        [Local::now().to_rfc3339()],
    )?;
    tx.execute("UPDATE services SET status='interrupted' WHERE status IN ('running','stopping','restarting')",[])?;
    tx.commit()?;
    Ok(())
}

impl Store {
    pub fn services(&self) -> Result<Vec<Service>> {
        let mut stmt=self.0.prepare("SELECT s.id,s.name,s.command,s.cwd,s.powershell,s.load_profile,s.autostart,
            s.restart_on_failure,s.restart_delay,s.max_restarts,s.status,
            COALESCE((SELECT started FROM service_runs WHERE service_id=s.id ORDER BY id DESC LIMIT 1),''),
            (SELECT exit_code FROM service_runs WHERE service_id=s.id ORDER BY id DESC LIMIT 1)
            FROM services s ORDER BY s.id DESC")?;
        Ok(stmt
            .query_map([], |r| {
                Ok(Service {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    command: r.get(2)?,
                    cwd: r.get(3)?,
                    powershell: r.get(4)?,
                    load_profile: r.get(5)?,
                    autostart: r.get(6)?,
                    restart_on_failure: r.get(7)?,
                    restart_delay: r.get(8)?,
                    max_restarts: r.get(9)?,
                    status: r.get(10)?,
                    started: r.get(11)?,
                    exit_code: r.get(12)?,
                    ..Service::default()
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn save_service(&self, s: &Service) -> Result<()> {
        validate(s)?;
        let values = params![
            s.name.trim(),
            s.command.trim(),
            s.cwd,
            s.powershell,
            s.load_profile,
            s.autostart,
            s.restart_on_failure,
            s.restart_delay,
            s.max_restarts,
            s.id
        ];
        if s.id==0 {
            self.0.execute("INSERT INTO services(name,command,cwd,powershell,load_profile,autostart,restart_on_failure,restart_delay,max_restarts)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![s.name.trim(),s.command.trim(),s.cwd,s.powershell,s.load_profile,
                s.autostart,s.restart_on_failure,s.restart_delay,s.max_restarts])?;
        } else if self.0.execute("UPDATE services SET name=?1,command=?2,cwd=?3,powershell=?4,load_profile=?5,autostart=?6,
            restart_on_failure=?7,restart_delay=?8,max_restarts=?9 WHERE id=?10", values)?==0 { bail!("服务已不存在"); }
        Ok(())
    }

    fn service_status(&self, id: i64, status: &str) -> Result<()> {
        self.0.execute(
            "UPDATE services SET status=?1 WHERE id=?2",
            params![status, id],
        )?;
        Ok(())
    }

    fn begin_service(&self, s: &Service, cwd: &str, trigger: &str) -> Result<i64> {
        self.0.execute(
            "INSERT INTO service_runs(service_id,name,command,cwd,trigger,started,status)
            VALUES(?1,?2,?3,?4,?5,?6,'running')",
            params![
                s.id,
                s.name,
                s.command,
                cwd,
                trigger,
                Local::now().to_rfc3339()
            ],
        )?;
        Ok(self.0.last_insert_rowid())
    }

    pub fn append_service(&self, run: i64, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let tx = self.0.unchecked_transaction()?;
        let mut remaining = text;
        while !remaining.is_empty() {
            let mut end = remaining.len().min(64 * 1024);
            while !remaining.is_char_boundary(end) {
                end -= 1;
            }
            tx.execute(
                "INSERT INTO service_output(run_id,text,bytes) VALUES(?1,?2,?3)",
                params![run, &remaining[..end], end as i64],
            )?;
            remaining = &remaining[end..];
        }
        tx.commit()?;
        Ok(())
    }

    pub fn prune_service_output(&self, service: i64) -> Result<()> {
        self.0.execute(
            "DELETE FROM service_output WHERE id IN (
            SELECT id FROM (
                SELECT o.id,SUM(o.bytes) OVER (ORDER BY o.id DESC) AS total
                FROM service_output o JOIN service_runs r ON r.id=o.run_id WHERE r.service_id=?1
            ) WHERE total>?2)",
            params![service, RETAIN_BYTES],
        )?;
        Ok(())
    }

    fn finish_service(&self, run: i64, status: &str, code: Option<i32>) -> Result<()> {
        self.0.execute(
            "UPDATE service_runs SET status=?1,exit_code=?2,finished=?3 WHERE id=?4",
            params![status, code, Local::now().to_rfc3339(), run],
        )?;
        Ok(())
    }

    pub fn service_logs(&self, service: Option<i64>, before: i64) -> Result<Vec<Run>> {
        let mut stmt=self.0.prepare("SELECT id,name,command,cwd,trigger,started,COALESCE(finished,''),status,exit_code
            FROM service_runs WHERE (?1 IS NULL OR service_id=?1) AND (?2=0 OR id<?2) ORDER BY id DESC LIMIT 50")?;
        Ok(stmt
            .query_map(params![service, before], |r| {
                Ok(Run {
                    id: r.get(0)?,
                    task_name: r.get(1)?,
                    command: r.get(2)?,
                    cwd: r.get(3)?,
                    trigger: r.get(4)?,
                    started: r.get(5)?,
                    finished: r.get(6)?,
                    status: r.get(7)?,
                    exit_code: r.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn service_output(&self, run: i64) -> Result<String> {
        let mut stmt = self
            .0
            .prepare("SELECT text FROM service_output WHERE run_id=?1 ORDER BY id DESC")?;
        let mut rows = stmt.query([run])?;
        let mut chunks = Vec::new();
        let mut bytes = 0;
        while let Some(row) = rows.next()? {
            let text: String = row.get(0)?;
            bytes += text.len();
            chunks.push(text);
            if bytes >= VIEW_BYTES {
                break;
            }
        }
        chunks.reverse();
        let mut text = chunks.concat();
        if text.len() > VIEW_BYTES {
            let mut start = text.len() - VIEW_BYTES;
            while !text.is_char_boundary(start) {
                start += 1;
            }
            text.drain(..start);
        }
        Ok(text)
    }
}

pub enum ServiceAction {
    Save(Service),
    Start(i64),
    Stop(i64),
    Restart(i64),
    Delete(i64),
}

struct Active {
    process: Running,
    since: Instant,
    stopping: Option<Instant>,
    restart: bool,
    unpruned: usize,
}

pub struct ServiceManager {
    active: HashMap<i64, Active>,
    retries: HashMap<i64, u32>,
    retry_at: HashMap<i64, Instant>,
    tx: SyncSender<PipeEvent>,
    rx: Receiver<PipeEvent>,
}

impl ServiceManager {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::sync_channel(64);
        Self {
            active: HashMap::new(),
            retries: HashMap::new(),
            retry_at: HashMap::new(),
            tx,
            rx,
        }
    }

    pub fn snapshot(&self, store: &Store) -> Result<Vec<Service>> {
        let mut services = store.services()?;
        for s in &mut services {
            s.restarts = self.retries.get(&s.id).copied().unwrap_or(0);
            if let Some(active) = self.active.get(&s.id) {
                s.pid = Some(active.process.child.id());
                s.uptime = active.since.elapsed().as_secs();
            }
        }
        Ok(services)
    }

    pub fn has_active(&self, owner: Option<i64>) -> bool {
        self.active
            .keys()
            .chain(self.retry_at.keys())
            .any(|id| owner.is_none_or(|owner| owner == *id))
    }

    pub fn autostart(&mut self, store: &Store, data: &Path, default: &Path) -> Result<()> {
        for s in store.services()?.iter().filter(|s| s.autostart) {
            self.launch(s, "autostart", store, data, default)?;
        }
        Ok(())
    }

    pub fn action(
        &mut self,
        action: ServiceAction,
        store: &Store,
        data: &Path,
        default: &Path,
    ) -> Result<bool> {
        match action {
            ServiceAction::Save(s) => {
                if self.active.contains_key(&s.id) || self.retry_at.contains_key(&s.id) {
                    bail!("请先停止服务，再修改配置；修改后重新启动生效");
                }
                store.save_service(&s)?;
                return Ok(true);
            }
            ServiceAction::Delete(id) => {
                if self.active.contains_key(&id) || self.retry_at.contains_key(&id) {
                    bail!("请先停止服务再删除");
                }
                let tx = store.0.unchecked_transaction()?;
                tx.execute("DELETE FROM service_runs WHERE service_id=?1", [id])?;
                tx.execute("DELETE FROM services WHERE id=?1", [id])?;
                tx.commit()?;
                self.retries.remove(&id);
            }
            ServiceAction::Stop(id) => self.stop(id, false, store)?,
            ServiceAction::Restart(id) => {
                self.retries.remove(&id);
                if self.active.contains_key(&id) {
                    self.stop(id, true, store)?;
                } else {
                    self.retry_at.remove(&id);
                    self.start_id(id, "restart", store, data, default)?;
                }
            }
            ServiceAction::Start(id) => {
                if self.active.contains_key(&id) || self.retry_at.contains_key(&id) {
                    bail!("服务已经启动或正在等待重试，请勿重复启动");
                }
                self.retries.remove(&id);
                self.start_id(id, "manual", store, data, default)?;
            }
        }
        Ok(false)
    }

    fn start_id(
        &mut self,
        id: i64,
        trigger: &str,
        store: &Store,
        data: &Path,
        default: &Path,
    ) -> Result<()> {
        let service = store
            .services()?
            .into_iter()
            .find(|s| s.id == id)
            .context("服务已不存在")?;
        self.launch(&service, trigger, store, data, default)
    }

    fn launch(
        &mut self,
        service: &Service,
        trigger: &str,
        store: &Store,
        data: &Path,
        default: &Path,
    ) -> Result<()> {
        let cwd = if service.cwd.is_empty() {
            default.to_string_lossy().into_owned()
        } else {
            service.cwd.clone()
        };
        let id = store.begin_service(service, &cwd, trigger)?;
        let scripts = data.join("service-scripts");
        let result = (|| -> Result<Running> {
            std::fs::create_dir_all(&scripts)?;
            let command = if service.powershell {
                let script = scripts.join(format!("run-{id}.ps1"));
                let contents = format!(
                    "$ErrorActionPreference = 'Stop'\nSet-Location -LiteralPath '{}'\n$global:LASTEXITCODE = 0\n{}\nexit $LASTEXITCODE\n",
                    cwd.replace('\'', "''"),
                    service.command
                );
                std::fs::write(&script, contents)?;
                let profile = if service.load_profile {
                    ""
                } else {
                    "-NoProfile "
                };
                #[cfg(windows)]
                let invocation = format!(
                    "pwsh.exe -NoLogo -NonInteractive {profile}-File \"{}\"",
                    script.display()
                );
                #[cfg(unix)]
                let invocation = format!(
                    "pwsh -NoLogo -NonInteractive {profile}-File '{}'",
                    script.to_string_lossy().replace('\'', "'\"'\"'")
                );
                invocation
            } else {
                service.command.clone()
            };
            spawn(
                &Task {
                    command,
                    ..Task::default()
                },
                &cwd,
                id,
                &scripts,
            )
        })();
        match result {
            Ok(mut process) => {
                read_pipe(
                    process.child.stdout.take().unwrap(),
                    id,
                    false,
                    self.tx.clone(),
                );
                read_pipe(
                    process.child.stderr.take().unwrap(),
                    id,
                    true,
                    self.tx.clone(),
                );
                self.active.insert(
                    service.id,
                    Active {
                        process,
                        since: Instant::now(),
                        stopping: None,
                        restart: false,
                        unpruned: 0,
                    },
                );
                store.service_status(service.id, "running")?;
            }
            Err(e) => {
                store.append_service(id, &format!("[系统] 服务启动失败：{e:#}\n"))?;
                store.finish_service(id, "failed", None)?;
                store.service_status(service.id, "failed")?;
                self.schedule_retry(service, store)?;
                remove_scripts(data, id);
            }
        }
        Ok(())
    }

    fn schedule_retry(&mut self, service: &Service, store: &Store) -> Result<()> {
        let retries = self.retries.entry(service.id).or_default();
        if service.restart_on_failure && *retries < service.max_restarts {
            *retries += 1;
            self.retry_at.insert(
                service.id,
                Instant::now() + Duration::from_secs(u64::from(service.restart_delay)),
            );
            store.service_status(service.id, "restarting")?;
        }
        Ok(())
    }

    fn stop(&mut self, id: i64, restart: bool, store: &Store) -> Result<()> {
        self.retry_at.remove(&id);
        if let Some(active) = self.active.get_mut(&id) {
            active.restart = restart;
            if active.stopping.is_none() {
                store.append_service(
                    active.process.run_id,
                    "\n[系统] 请求停止服务，5 秒后仍未结束将强制终止进程树。\n",
                )?;
                active.process.request_stop();
                active.stopping = Some(Instant::now() + Duration::from_secs(5));
                store.service_status(id, "stopping")?;
            }
        } else {
            store.service_status(id, "stopped")?;
        }
        Ok(())
    }

    fn collect_output(&mut self) {
        for _ in 0..128 {
            let Ok(event) = self.rx.try_recv() else {
                break;
            };
            match event {
                PipeEvent::Data(id, text) => {
                    if let Some(a) = self.active.values_mut().find(|a| a.process.run_id == id) {
                        a.process.pending.push_str(&text);
                    }
                }
                PipeEvent::Closed(id) => {
                    if let Some(a) = self.active.values_mut().find(|a| a.process.run_id == id) {
                        a.process.closed += 1;
                    }
                }
            }
        }
    }

    pub fn tick(&mut self, store: &Store, data: &Path, default: &Path) -> Result<()> {
        self.collect_output();
        let mut finished = Vec::new();
        for (&id, active) in &mut self.active {
            let run = &mut active.process;
            if !run.pending.is_empty() {
                store.append_service(run.run_id, &run.pending)?;
                active.unpruned += run.pending.len();
                run.pending.clear();
                if active.unpruned >= 1024 * 1024 {
                    store.prune_service_output(id)?;
                    active.unpruned = 0;
                }
            }
            if run.exit.is_none()
                && let Some(exit) = run.child.try_wait()?
            {
                run.exit = Some(exit);
                run.kill_tree();
            }
            if active
                .stopping
                .is_some_and(|deadline| Instant::now() >= deadline)
            {
                run.kill_tree();
            }
            if run.closed == 2
                && let Some(exit) = run.exit
            {
                let status = if active.stopping.is_some() {
                    "stopped"
                } else if exit.success() {
                    "exited"
                } else {
                    "failed"
                };
                store.finish_service(run.run_id, status, exit.code())?;
                store.service_status(id, status)?;
                store.prune_service_output(id)?;
                finished.push((id, active.restart, status == "failed"));
            }
        }
        for (id, restart, failed) in finished {
            if let Some(active) = self.active.remove(&id) {
                remove_scripts(data, active.process.run_id);
            }
            if restart {
                self.start_id(id, "restart", store, data, default)?;
            } else if failed
                && let Some(service) = store.services()?.into_iter().find(|s| s.id == id)
            {
                self.schedule_retry(&service, store)?;
            }
        }
        let due: Vec<_> = self
            .retry_at
            .iter()
            .filter(|(_, at)| Instant::now() >= **at)
            .map(|(&id, _)| id)
            .collect();
        for id in due {
            self.retry_at.remove(&id);
            self.start_id(id, "retry", store, data, default)?;
        }
        Ok(())
    }

    pub fn shutdown(&mut self, store: &Store, data: &Path) {
        for &id in self.retry_at.keys() {
            let _ = store.service_status(id, "stopped");
        }
        self.retry_at.clear();
        for active in self.active.values_mut() {
            active.restart = false;
            active.process.request_stop();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && self.active.values().any(|a| a.process.exit.is_none()) {
            self.collect_output();
            for active in self.active.values_mut() {
                if active.process.exit.is_none() {
                    active.process.exit = active.process.child.try_wait().ok().flatten();
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        for active in self.active.values_mut() {
            active.process.kill_tree();
            let _ = active.process.child.wait();
        }
        // Drain pipe readers after termination so final shutdown messages are retained.
        let drain_until = Instant::now() + Duration::from_secs(1);
        while Instant::now() < drain_until && self.active.values().any(|a| a.process.closed < 2) {
            self.collect_output();
            std::thread::sleep(Duration::from_millis(10));
        }
        for (&id, active) in &self.active {
            let run = active.process.run_id;
            let _ = store.append_service(run, &active.process.pending);
            let _ = store.append_service(run, "\n[系统] 应用退出，服务已停止。\n");
            let _ =
                store.finish_service(run, "stopped", active.process.exit.and_then(|e| e.code()));
            let _ = store.service_status(id, "stopped");
            let _ = store.prune_service_output(id);
            remove_scripts(data, run);
        }
        self.active.clear();
    }
}

fn remove_scripts(data: &Path, run: i64) {
    for extension in ["cmd", "ps1"] {
        let _ = std::fs::remove_file(
            data.join("service-scripts")
                .join(format!("run-{run}.{extension}")),
        );
    }
}
