use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local};
use croner::Cron;
use rusqlite::{Connection, OptionalExtension, params};
use std::{path::Path, str::FromStr, time::Duration};

#[derive(Clone, Debug, Default)]
pub struct Task {
    pub id: i64,
    pub name: String,
    pub cron: String,
    pub command: String,
    pub cwd: String,
    pub enabled: bool,
    pub next: String,
    pub last: String,
    pub running: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Run {
    pub id: i64,
    pub task_name: String,
    pub command: String,
    pub cwd: String,
    pub trigger: String,
    pub started: String,
    pub finished: String,
    pub status: String,
    pub exit_code: Option<i32>,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub tasks: Vec<Task>,
    pub services: Vec<crate::services::Service>,
    pub success: i64,
    pub failed: i64,
    pub running: usize,
}

pub fn parse_cron(value: &str) -> Result<Cron> {
    if value.split_whitespace().count() != 5 {
        bail!("请输入五段 cron：分 时 日 月 周，例如 */5 * * * *");
    }
    Cron::from_str(value).context("cron 规则无效")
}

pub fn next_time(value: &str, after: DateTime<Local>) -> Result<DateTime<Local>> {
    parse_cron(value)?
        .find_next_occurrence(&after, false)
        .context("规则没有可用的下一次执行时间")
}

pub fn validate(task: &Task) -> Result<()> {
    if task.name.trim().is_empty() || task.name.chars().count() > 100 {
        bail!("任务名称不能为空，且不能超过 100 个字符");
    }
    if task.command.trim().is_empty() || task.command.contains('\0') {
        bail!("请填写有效的脚本命令");
    }
    next_time(&task.cron, Local::now())?;
    if !task.cwd.is_empty()
        && (!Path::new(&task.cwd).is_absolute() || !Path::new(&task.cwd).is_dir())
    {
        bail!("执行目录必须是已存在的绝对路径");
    }
    Ok(())
}

pub struct Store(pub Connection);

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(5))?;
        crate::services::migrate(&db, path)?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS tasks (
               id INTEGER PRIMARY KEY, name TEXT NOT NULL, cron TEXT NOT NULL,
               command TEXT NOT NULL, cwd TEXT NOT NULL, enabled INTEGER NOT NULL,
               created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             );
             CREATE TABLE IF NOT EXISTS runs (
               id INTEGER PRIMARY KEY, task_id INTEGER REFERENCES tasks(id) ON DELETE SET NULL,
               task_name TEXT NOT NULL, command TEXT NOT NULL, cwd TEXT NOT NULL,
               trigger TEXT NOT NULL, started TEXT NOT NULL, finished TEXT,
               status TEXT NOT NULL, exit_code INTEGER, output TEXT NOT NULL DEFAULT ''
             );
             CREATE INDEX IF NOT EXISTS runs_task ON runs(task_id, id DESC);
             CREATE INDEX IF NOT EXISTS runs_status ON runs(status);",
        )?;
        // A previous process may have been terminated without a graceful shutdown.
        db.execute(
            "UPDATE runs SET status='interrupted', finished=?1,
             output=output || '\n[系统] 应用异常退出或电脑重启，执行结果未知。\n'
             WHERE status='running'",
            [Local::now().to_rfc3339()],
        )?;
        Ok(Self(db))
    }

    pub fn tasks(&self) -> Result<Vec<Task>> {
        let mut stmt = self.0.prepare(
            "SELECT t.id,t.name,t.cron,t.command,t.cwd,t.enabled,
             COALESCE((SELECT status FROM runs WHERE task_id=t.id ORDER BY id DESC LIMIT 1),'')
             FROM tasks t ORDER BY t.id DESC",
        )?;
        Ok(stmt
            .query_map([], |r| {
                Ok(Task {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    cron: r.get(2)?,
                    command: r.get(3)?,
                    cwd: r.get(4)?,
                    enabled: r.get(5)?,
                    last: r.get(6)?,
                    ..Task::default()
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn save(&self, task: &Task) -> Result<()> {
        validate(task)?;
        if task.id == 0 {
            self.0.execute(
                "INSERT INTO tasks(name,cron,command,cwd,enabled) VALUES(?1,?2,?3,?4,?5)",
                params![
                    task.name.trim(),
                    task.cron.trim(),
                    task.command.trim(),
                    task.cwd,
                    task.enabled
                ],
            )?;
        } else if self.0.execute(
            "UPDATE tasks SET name=?1,cron=?2,command=?3,cwd=?4,enabled=?5 WHERE id=?6",
            params![
                task.name.trim(),
                task.cron.trim(),
                task.command.trim(),
                task.cwd,
                task.enabled,
                task.id
            ],
        )? == 0
        {
            bail!("任务已不存在");
        }
        Ok(())
    }

    pub fn begin_run(&self, task: &Task, trigger: &str, cwd: &str) -> Result<i64> {
        self.0.execute(
            "INSERT INTO runs(task_id,task_name,command,cwd,trigger,started,status)
            VALUES(?1,?2,?3,?4,?5,?6,'running')",
            params![
                task.id,
                task.name,
                task.command,
                cwd,
                trigger,
                Local::now().to_rfc3339()
            ],
        )?;
        Ok(self.0.last_insert_rowid())
    }

    pub fn append(&self, id: i64, output: &str) -> Result<()> {
        self.0.execute(
            "UPDATE runs SET output=output || ?1 WHERE id=?2",
            params![output, id],
        )?;
        Ok(())
    }

    pub fn finish(&self, id: i64, status: &str, code: Option<i32>) -> Result<()> {
        self.0.execute(
            "UPDATE runs SET status=?1,exit_code=?2,finished=?3 WHERE id=?4",
            params![status, code, Local::now().to_rfc3339(), id],
        )?;
        Ok(())
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let (success, failed) = self.0.query_row(
            "SELECT COALESCE(SUM(status='success'),0),
             COALESCE(SUM(status IN ('failed','interrupted')),0) FROM runs",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(Snapshot {
            tasks: self.tasks()?,
            success,
            failed,
            running: 0,
            services: Vec::new(),
        })
    }

    pub fn logs(&self, task_id: Option<i64>, before: i64) -> Result<Vec<Run>> {
        let mut stmt = self.0.prepare(
            "SELECT id,task_name,command,cwd,trigger,started,
            COALESCE(finished,''),status,exit_code FROM runs
            WHERE (?1 IS NULL OR task_id=?1) AND (?2=0 OR id<?2) ORDER BY id DESC LIMIT 50",
        )?;
        Ok(stmt
            .query_map(params![task_id, before], |r| {
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

    pub fn clear_logs(&self, service: bool, owner: Option<i64>) -> Result<()> {
        self.0.execute(
            if service {
                "DELETE FROM service_runs WHERE (?1 IS NULL OR service_id=?1)"
            } else {
                "DELETE FROM runs WHERE (?1 IS NULL OR task_id=?1)"
            },
            [owner],
        )?;
        Ok(())
    }

    pub fn output(&self, id: i64) -> Result<String> {
        Ok(self
            .0
            .query_row("SELECT output FROM runs WHERE id=?1", [id], |r| r.get(0))
            .optional()?
            .unwrap_or_default())
    }
}
