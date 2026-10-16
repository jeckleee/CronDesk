use crate::{
    engine::{Action, Engine, Event},
    services::{Service, ServiceAction},
    store::{Run, Snapshot, Task, next_time, validate},
};
use chrono::{DateTime, Local};
use eframe::egui::{self, Align, Color32, FontData, FontFamily, FontId, RichText, Stroke, Vec2};
use egui_extras::{Column, TableBuilder};
use std::{
    fs::File,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};
use tray_icon::{
    TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem},
};

const INK: Color32 = Color32::from_rgb(29, 45, 50);
const MUTED: Color32 = Color32::from_rgb(115, 129, 133);
const GREEN: Color32 = Color32::from_rgb(21, 121, 94);
const RED: Color32 = Color32::from_rgb(190, 66, 72);
const BORDER: Color32 = Color32::from_rgb(225, 232, 230);

#[path = "service_ui.rs"]
mod service_ui;

enum TrayAction {
    Show,
    Quit,
}
enum Confirm {
    Delete(i64, String),
    DeleteService(i64, String),
    ClearLogs(bool, Option<i64>, String),
    Quit,
}

struct LogView {
    service: bool,
    task_id: Option<i64>,
    title: String,
    runs: Vec<Run>,
    selected: Option<i64>,
    output: String,
    before: i64,
    pages: Vec<i64>,
    refreshed: Instant,
}

pub struct App {
    engine: Engine,
    _lock: File,
    data_dir: PathBuf,
    default_dir: PathBuf,
    snapshot: Snapshot,
    editor: Option<Task>,
    service_editor: Option<Service>,
    service_tab: bool,
    service_search: String,
    saving: bool,
    logs: Option<LogView>,
    confirm: Option<Confirm>,
    notice: Option<(String, bool, Instant)>,
    search: String,
    filter: usize,
    tray: Option<TrayIcon>,
    tray_events: mpsc::Receiver<TrayAction>,
    quitting: bool,
    scheduler_alive: bool,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        engine: Engine,
        lock: File,
        data_dir: PathBuf,
        default_dir: PathBuf,
    ) -> Self {
        let ctx = &cc.egui_ctx;
        let mut fonts = egui::FontDefinitions::default();
        let candidates = if cfg!(windows) {
            let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
            vec![
                format!("{windows}\\Fonts\\msyh.ttc"),
                format!("{windows}\\Fonts\\simhei.ttf"),
            ]
        } else {
            vec![
                "/System/Library/Fonts/STHeiti Light.ttc".into(),
                "/System/Library/Fonts/PingFang.ttc".into(),
            ]
        };
        for path in candidates {
            if let Ok(bytes) = std::fs::read(path) {
                fonts
                    .font_data
                    .insert("chinese".into(), FontData::from_owned(bytes).into());
                fonts
                    .families
                    .get_mut(&FontFamily::Proportional)
                    .unwrap()
                    .push("chinese".into());
                fonts
                    .families
                    .get_mut(&FontFamily::Monospace)
                    .unwrap()
                    .push("chinese".into());
                break;
            }
        }
        ctx.set_fonts(fonts);
        let mut style = (*ctx.style()).clone();
        style.visuals = egui::Visuals::light();
        style.visuals.override_text_color = Some(INK);
        style.visuals.panel_fill = Color32::from_rgb(246, 248, 247);
        style.visuals.window_fill = Color32::WHITE;
        style.visuals.selection.bg_fill = Color32::from_rgb(214, 239, 229);
        style.visuals.selection.stroke = Stroke::new(1.0_f32, GREEN);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(239, 244, 241);
        style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(239, 244, 241);
        style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
        style.spacing.item_spacing = Vec2::new(10.0, 10.0);
        style.spacing.button_padding = Vec2::new(12.0, 8.0);
        style.spacing.interact_size.y = 32.0;
        style
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(14.0));
        ctx.set_style(style);
        let (tx, tray_events) = mpsc::channel();
        let tray = create_tray(cc, tx);
        let notice = tray.as_ref().err().map(|e| {
            (
                format!("托盘不可用：{e}。关闭窗口会退出程序。"),
                true,
                Instant::now(),
            )
        });
        Self {
            engine,
            _lock: lock,
            data_dir,
            default_dir,
            snapshot: Snapshot::default(),
            editor: None,
            service_editor: None,
            service_tab: false,
            service_search: String::new(),
            saving: false,
            logs: None,
            confirm: None,
            notice,
            search: String::new(),
            filter: 0,
            tray: tray.ok(),
            tray_events,
            quitting: false,
            scheduler_alive: true,
        }
    }

    fn receive(&mut self) {
        while let Ok(snapshot) = self.engine.snapshots.try_recv() {
            self.snapshot = snapshot;
        }
        while let Ok(event) = self.engine.rx.try_recv() {
            match event {
                Event::Saved => {
                    self.editor = None;
                    self.saving = false;
                    self.tell("任务已保存，调度已更新", false);
                }
                Event::ServiceSaved => {
                    self.service_editor = None;
                    self.saving = false;
                    self.tell("服务已保存，点击启动开始运行", false);
                }
                Event::Notice(text) => self.tell(&text, false),
                Event::Error(text) => {
                    self.saving = false;
                    self.tell(&text, true);
                }
                Event::Halted(text) => {
                    self.scheduler_alive = false;
                    self.saving = false;
                    self.tell(&text, true);
                }
                Event::Logs(service, owner, before, runs) => {
                    if let Some(logs) = &mut self.logs
                        && logs.service == service
                        && logs.task_id == owner
                        && logs.before == before
                    {
                        if logs.selected.is_none()
                            && let Some(run) = runs.first()
                        {
                            logs.selected = Some(run.id);
                            self.engine.send(Action::Output(logs.service, run.id));
                        }
                        logs.runs = runs;
                    }
                }
                Event::Output(service, id, output) => {
                    if let Some(logs) = &mut self.logs
                        && logs.selected == Some(id)
                        && logs.service == service
                    {
                        logs.output = output;
                    }
                }
                Event::LogsCleared(service, owner) => {
                    if let Some(logs) = &mut self.logs
                        && logs.service == service
                        && (owner.is_none() || logs.task_id.is_none() || logs.task_id == owner)
                    {
                        logs.runs.clear();
                        logs.selected = None;
                        logs.output.clear();
                        logs.before = 0;
                        logs.pages.clear();
                        logs.refreshed = Instant::now();
                        self.engine.send(Action::Logs(service, logs.task_id, 0));
                    }
                }
            }
        }
    }

    fn tell(&mut self, text: &str, error: bool) {
        self.notice = Some((text.into(), error, Instant::now()));
    }

    fn open_logs(&mut self, id: Option<i64>, title: String) {
        self.open_log_view(false, id, title);
    }

    fn open_log_view(&mut self, service: bool, id: Option<i64>, title: String) {
        self.logs = Some(LogView {
            service,
            task_id: id,
            title,
            runs: Vec::new(),
            selected: None,
            output: String::new(),
            before: 0,
            pages: Vec::new(),
            refreshed: Instant::now(),
        });
        self.engine.send(Action::Logs(service, id, 0));
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(38.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 10, GREEN);
            ui.painter()
                .circle_stroke(rect.center(), 11.0, Stroke::new(1.8_f32, Color32::WHITE));
            ui.painter().line_segment(
                [rect.center() + Vec2::new(0.0, -7.0), rect.center()],
                Stroke::new(1.8_f32, Color32::WHITE),
            );
            ui.painter().line_segment(
                [rect.center(), rect.center() + Vec2::new(6.0, 3.0)],
                Stroke::new(1.8_f32, Color32::WHITE),
            );
            ui.vertical(|ui| {
                ui.label(RichText::new("CronDesk").size(22.0).strong());
                ui.label(
                    RichText::new("让每一项任务，准时发生")
                        .size(11.0)
                        .color(MUTED),
                );
            });
            ui.add_space(20.0);
            if ui
                .add(primary(if self.service_tab {
                    "＋ 新建服务"
                } else {
                    "＋ 新建任务"
                }))
                .clicked()
            {
                self.notice = None;
                if self.service_tab {
                    self.service_editor = Some(Service::default());
                } else {
                    self.editor = Some(Task {
                        cron: "*/5 * * * *".into(),
                        enabled: true,
                        ..Task::default()
                    });
                }
            }
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if self.service_tab {
                    stat(
                        ui,
                        "异常退出",
                        self.snapshot
                            .services
                            .iter()
                            .filter(|s| matches!(s.status.as_str(), "failed" | "interrupted"))
                            .count() as i64,
                        RED,
                    );
                    stat(
                        ui,
                        "运行中",
                        self.snapshot
                            .services
                            .iter()
                            .filter(|s| s.pid.is_some())
                            .count() as i64,
                        GREEN,
                    );
                    stat(ui, "服务总数", self.snapshot.services.len() as i64, INK);
                } else {
                    stat(ui, "执行失败", self.snapshot.failed, RED);
                    stat(ui, "执行成功", self.snapshot.success, GREEN);
                    stat(ui, "当前任务", self.snapshot.tasks.len() as i64, INK);
                }
            });
        });
    }

    fn list(&mut self, ui: &mut egui::Ui) {
        ui.add_space(26.0);
        ui.horizontal(|ui| {
            ui.heading("任务列表");
            ui.label(
                RichText::new(format!("{} 项运行中", self.snapshot.running))
                    .size(12.0)
                    .color(GREEN),
            );
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if ui.button("执行日志").clicked() {
                    self.open_logs(None, "全部执行日志".into());
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("搜索任务名称或命令")
                        .desired_width(220.0),
                );
            });
        });
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            for (i, text) in ["全部任务", "已启用", "已暂停"].iter().enumerate() {
                ui.selectable_value(&mut self.filter, i, *text);
            }
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new("本地调度 · SQLite 持久化")
                        .size(12.0)
                        .color(MUTED),
                );
            });
        });
        ui.add_space(14.0);
        let search = self.search.to_lowercase();
        let tasks: Vec<_> = self
            .snapshot
            .tasks
            .iter()
            .filter(|t| {
                (self.filter == 0
                    || (self.filter == 1 && t.enabled)
                    || (self.filter == 2 && !t.enabled))
                    && (t.name.to_lowercase().contains(&search)
                        || t.command.to_lowercase().contains(&search))
            })
            .cloned()
            .collect();
        egui::Frame::new()
            .fill(Color32::WHITE)
            .stroke(Stroke::new(1.0_f32, BORDER))
            .corner_radius(10)
            .inner_margin(16)
            .show(ui, |ui| {
                if tasks.is_empty() {
                    ui.set_min_height(350.0);
                    ui.vertical_centered(|ui| {
                        ui.add_space(90.0);
                        ui.label(
                            RichText::new(if self.snapshot.tasks.is_empty() {
                                "从你的第一个定时任务开始"
                            } else {
                                "没有匹配的任务"
                            })
                            .size(21.0)
                            .strong(),
                        );
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new("填写 cron 规则和脚本命令，剩下的交给 CronDesk。")
                                .color(MUTED),
                        );
                        ui.add_space(18.0);
                        if self.snapshot.tasks.is_empty()
                            && ui.add(primary("＋ 创建第一个任务")).clicked()
                        {
                            self.editor = Some(Task {
                                cron: "*/5 * * * *".into(),
                                enabled: true,
                                ..Task::default()
                            });
                        }
                    });
                    return;
                }
                let table_height = (ui.available_height() - 95.0).max(170.0);
                TableBuilder::new(ui)
                    .striped(true)
                    .cell_layout(egui::Layout::left_to_right(Align::Center))
                    .column(Column::remainder().at_least(180.0))
                    .column(Column::exact(130.0))
                    .column(Column::exact(124.0))
                    .column(Column::exact(76.0))
                    .column(Column::exact(244.0))
                    .min_scrolled_height(170.0)
                    .max_scroll_height(table_height)
                    .header(32.0, |mut row| {
                        for title in [
                            "任务 / 脚本命令",
                            "CRON 规则",
                            "下次执行",
                            "最近结果",
                            "操作",
                        ] {
                            row.col(|ui| {
                                ui.label(RichText::new(title).size(12.0).color(MUTED));
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(78.0, tasks.len(), |mut row| {
                            let task = &tasks[row.index()];
                            row.col(|ui| {
                                ui.vertical(|ui| {
                                    ui.add_space(10.0);
                                    ui.add(
                                        egui::Label::new(RichText::new(&task.name).strong())
                                            .truncate(),
                                    )
                                    .on_hover_text(&task.name);
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(
                                                task.command.lines().next().unwrap_or(""),
                                            )
                                            .size(11.0)
                                            .monospace()
                                            .color(MUTED),
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(&task.command);
                                });
                            });
                            row.col(|ui| {
                                ui.label(RichText::new(&task.cron).monospace().size(12.0));
                            });
                            row.col(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(if task.enabled { &task.next } else { "—" });
                                    ui.label(
                                        RichText::new(if task.enabled {
                                            "已启用"
                                        } else {
                                            "已暂停"
                                        })
                                        .size(11.0)
                                        .color(if task.enabled { GREEN } else { MUTED }),
                                    );
                                });
                            });
                            row.col(|ui| {
                                status(ui, if task.running { "running" } else { &task.last });
                            });
                            row.col(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                ui.spacing_mut().button_padding = Vec2::new(7.0, 6.0);
                                if ui
                                    .button(if task.running { "停止" } else { "执行" })
                                    .clicked()
                                {
                                    self.engine.send(if task.running {
                                        Action::Stop(task.id)
                                    } else {
                                        Action::Run(task.id)
                                    });
                                }
                                if ui
                                    .button(if task.enabled { "暂停" } else { "启用" })
                                    .clicked()
                                {
                                    self.engine.send(Action::Toggle(task.id, !task.enabled));
                                }
                                if ui.button("日志").clicked() {
                                    self.open_logs(Some(task.id), task.name.clone());
                                }
                                ui.menu_button("更多", |ui| {
                                    if ui
                                        .add_enabled(!task.running, egui::Button::new("编辑任务"))
                                        .clicked()
                                    {
                                        self.editor = Some(task.clone());
                                        ui.close();
                                    }
                                    if ui
                                        .add_enabled(
                                            !task.running,
                                            egui::Button::new(RichText::new("删除任务").color(RED)),
                                        )
                                        .clicked()
                                    {
                                        self.confirm =
                                            Some(Confirm::Delete(task.id, task.name.clone()));
                                        ui.close();
                                    }
                                });
                            });
                        });
                    });
            });
        ui.add_space(12.0);
        ui.label(
            RichText::new("按电脑本地时区执行 · 同一任务不并发 · 睡眠或关机期间错过的计划不补跑")
                .size(12.0)
                .color(MUTED),
        );
    }

    fn editor(&mut self, ctx: &egui::Context) {
        let Some(task) = &mut self.editor else {
            return;
        };
        let mut cancel = false;
        let mut save = false;
        egui::Modal::new(egui::Id::new("task-editor")).show(ctx, |ui| {
            ui.set_width(580.0);
            ui.heading(if task.id == 0 {
                "新建定时任务"
            } else {
                "编辑定时任务"
            });
            ui.label(RichText::new("保存后自动调度，也可以随时手动执行。").color(MUTED));
            ui.add_space(10.0);
            ui.label("任务名称");
            ui.add(
                egui::TextEdit::singleline(&mut task.name)
                    .id_salt("task-name")
                    .hint_text("例如：每日数据备份")
                    .desired_width(f32::INFINITY),
            );
            ui.label("Cron 规则");
            ui.add(
                egui::TextEdit::singleline(&mut task.cron)
                    .id_salt("task-cron")
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal(|ui| {
                for (name, rule) in [
                    ("每分钟", "* * * * *"),
                    ("每 5 分钟", "*/5 * * * *"),
                    ("每天 09:00", "0 9 * * *"),
                    ("工作日 09:00", "0 9 * * 1-5"),
                ] {
                    if ui.small_button(name).clicked() {
                        task.cron = rule.into();
                    }
                }
            });
            ui.label(
                RichText::new("分  时  日  月  周  ·  支持 *、逗号、范围、步长；周日为 0 或 7")
                    .size(11.0)
                    .color(MUTED),
            );
            match next_time(&task.cron, Local::now()) {
                Ok(next) => {
                    ui.label(
                        RichText::new(format!(
                            "下次执行：{}",
                            next.format("%Y-%m-%d %H:%M:%S %:z")
                        ))
                        .size(12.0)
                        .color(GREEN),
                    );
                }
                Err(e) => {
                    ui.label(RichText::new(format!("{e:#}")).size(12.0).color(RED));
                }
            }
            ui.label("任务脚本命令");
            ui.add(
                egui::TextEdit::multiline(&mut task.command)
                    .id_salt("task-command")
                    .desired_rows(4)
                    .font(egui::TextStyle::Monospace)
                    .hint_text(if cfg!(windows) {
                        "python backup.py\r\n或 powershell -File backup.ps1"
                    } else {
                        "python3 backup.py\n或 /bin/sh backup.sh"
                    })
                    .desired_width(f32::INFINITY),
            );
            ui.label(
                RichText::new(if cfg!(windows) {
                    "使用 cmd.exe 执行；PowerShell 脚本请显式调用 powershell。"
                } else {
                    "使用 /bin/sh 执行；其他解释器请使用完整命令或绝对路径。"
                })
                .size(11.0)
                .color(MUTED),
            );
            ui.label("执行目录（可选）");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut task.cwd)
                        .id_salt("task-cwd")
                        .hint_text("留空使用用户主目录")
                        .desired_width(470.0),
                );
                if ui.button("选择…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .set_title("选择任务执行目录")
                        .pick_folder()
                {
                    task.cwd = path.to_string_lossy().into_owned();
                }
            });
            ui.label(
                RichText::new(format!("默认：{}", self.default_dir.display()))
                    .size(11.0)
                    .color(MUTED),
            );
            ui.checkbox(&mut task.enabled, "启用自动调度");
            if let Some((text, true, _)) = &self.notice {
                ui.colored_label(RED, text);
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("任务会以当前用户权限执行。")
                        .size(11.0)
                        .color(MUTED),
                );
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    save = ui
                        .add_enabled(
                            !self.saving,
                            primary(if self.saving {
                                "保存中…"
                            } else {
                                "保存任务"
                            }),
                        )
                        .clicked();
                    cancel = ui
                        .add_enabled(!self.saving, egui::Button::new("取消"))
                        .clicked();
                });
            });
        });
        if save {
            match validate(task) {
                Ok(()) => {
                    self.engine.send(Action::Save(task.clone()));
                    self.saving = true;
                }
                Err(e) => self.tell(&format!("{e:#}"), true),
            }
        }
        if cancel {
            self.editor = None;
        }
    }

    fn logs(&mut self, ctx: &egui::Context) {
        let Some(logs) = &mut self.logs else {
            return;
        };
        let mut open = true;
        let busy = if logs.service {
            self.snapshot.services.iter().any(|s| {
                logs.task_id.is_none_or(|id| id == s.id)
                    && (s.pid.is_some()
                        || matches!(s.status.as_str(), "running" | "stopping" | "restarting"))
            })
        } else {
            self.snapshot
                .tasks
                .iter()
                .any(|t| t.running && logs.task_id.is_none_or(|id| id == t.id))
        };
        if logs.refreshed.elapsed() >= Duration::from_secs(1) {
            self.engine
                .send(Action::Logs(logs.service, logs.task_id, logs.before));
            if let Some(id) = logs.selected {
                self.engine.send(Action::Output(logs.service, id));
            }
            logs.refreshed = Instant::now();
        }
        egui::Window::new(format!("执行日志 · {}", logs.title))
            .id(egui::Id::new("logs"))
            .open(&mut open)
            .default_size([960.0, 600.0])
            .min_width(800.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(if logs.service {
                        "每次启动单独记录，删除服务时同步删除日志。"
                    } else {
                        "每次执行单独记录，删除任务时同步删除日志。"
                    });
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_enabled(logs.runs.len() == 50, egui::Button::new("更早记录"))
                            .clicked()
                        {
                            logs.pages.push(logs.before);
                            logs.before = logs.runs.last().unwrap().id;
                            logs.selected = None;
                            self.engine
                                .send(Action::Logs(logs.service, logs.task_id, logs.before));
                        }
                        if ui
                            .add_enabled(!logs.pages.is_empty(), egui::Button::new("上一页"))
                            .clicked()
                        {
                            logs.before = logs.pages.pop().unwrap_or(0);
                            logs.selected = None;
                            self.engine
                                .send(Action::Logs(logs.service, logs.task_id, logs.before));
                        }
                    });
                });
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !busy && self.scheduler_alive,
                            egui::Button::new(RichText::new("清空日志").color(RED)),
                        )
                        .clicked()
                    {
                        self.confirm = Some(Confirm::ClearLogs(
                            logs.service,
                            logs.task_id,
                            logs.title.clone(),
                        ));
                    }
                    ui.weak(if busy {
                        "请先停止此范围内的任务或服务（含等待重试的服务）"
                    } else if logs.task_id.is_some() {
                        "清空当前对象的所有执行记录和输出，不影响其他对象"
                    } else if logs.service {
                        "清空全部服务的执行记录和输出，不影响定时任务日志"
                    } else {
                        "清空全部定时任务的执行记录和输出，不影响服务日志"
                    });
                });
                TableBuilder::new(ui)
                    .striped(true)
                    .column(Column::exact(180.0))
                    .column(Column::remainder())
                    .column(Column::exact(70.0))
                    .column(Column::exact(80.0))
                    .column(Column::exact(70.0))
                    .max_scroll_height(180.0)
                    .header(25.0, |mut row| {
                        for title in ["开始时间", "任务名称", "触发方式", "结果", "退出码"]
                        {
                            row.col(|ui| {
                                ui.weak(title);
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(32.0, logs.runs.len(), |mut row| {
                            let run = &logs.runs[row.index()];
                            row.set_selected(logs.selected == Some(run.id));
                            row.col(|ui| {
                                if ui
                                    .selectable_label(
                                        logs.selected == Some(run.id),
                                        format_time(&run.started),
                                    )
                                    .clicked()
                                {
                                    logs.selected = Some(run.id);
                                    logs.output.clear();
                                    self.engine.send(Action::Output(logs.service, run.id));
                                }
                            });
                            row.col(|ui| {
                                ui.add(egui::Label::new(&run.task_name).truncate());
                            });
                            row.col(|ui| {
                                ui.label(match run.trigger.as_str() {
                                    "manual" => "手动",
                                    "autostart" => "自动启动",
                                    "retry" => "异常重试",
                                    "restart" => "手动重启",
                                    _ => "计划",
                                });
                            });
                            row.col(|ui| status(ui, &run.status));
                            row.col(|ui| {
                                ui.label(
                                    run.exit_code
                                        .map(|v| v.to_string())
                                        .unwrap_or_else(|| "—".into()),
                                );
                            });
                        })
                    });
                ui.separator();
                if let Some(run) = logs.runs.iter().find(|r| Some(r.id) == logs.selected) {
                    ui.label(RichText::new(&run.command).monospace());
                    ui.label(
                        RichText::new(format!(
                            "目录：{}  |  结束：{}",
                            run.cwd,
                            if run.finished.is_empty() {
                                "—".into()
                            } else {
                                format_time(&run.finished)
                            }
                        ))
                        .size(12.0)
                        .color(MUTED),
                    );
                }
                ui.horizontal(|ui| {
                    ui.strong("执行输出");
                    ui.weak(if logs.service {
                        "服务持续记录 · 每个服务保留最近约 100 MiB · 此处显示最近 2 MiB"
                    } else {
                        "每次最多保存 2 MiB · 标准输出与错误输出"
                    });
                    if ui.small_button("复制日志").clicked() {
                        ui.ctx().copy_text(logs.output.clone());
                    }
                });
                egui::Frame::new()
                    .fill(Color32::from_rgb(27, 38, 43))
                    .corner_radius(6)
                    .inner_margin(12)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        egui::ScrollArea::both()
                            .max_height(280.0)
                            .min_scrolled_height(180.0)
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(if logs.output.is_empty() {
                                            "暂无输出；点击执行记录查看日志。"
                                        } else {
                                            &logs.output
                                        })
                                        .monospace()
                                        .size(12.0)
                                        .color(Color32::from_rgb(211, 231, 222)),
                                    )
                                    .selectable(true),
                                );
                            });
                    });
            });
        if !open {
            self.logs = None;
        }
    }

    fn confirmation(&mut self, ctx: &egui::Context) {
        let Some(confirm) = &self.confirm else {
            return;
        };
        let mut accept = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("confirm")).show(ctx, |ui| {
            ui.set_width(400.0);
            match confirm {
                Confirm::Delete(_, name) => {
                    ui.heading("删除任务");
                    ui.label(format!("确定删除「{name}」及其全部执行记录和日志？此操作无法撤销。"));
                }
                Confirm::DeleteService(_, name) => {
                    ui.heading("删除服务");
                    ui.label(format!(
                        "确定删除「{name}」及其全部启动记录和日志？此操作无法撤销。"
                    ));
                }
                Confirm::ClearLogs(service, owner, name) => {
                    ui.heading("清空日志");
                    let scope = if owner.is_some() {
                        format!("「{name}」")
                    } else if *service {
                        "全部服务".into()
                    } else {
                        "全部定时任务".into()
                    };
                    ui.label(format!("确定清空{scope}的全部执行记录和输出日志？此操作无法撤销，任务和服务配置会保留。"));
                    if !service {
                        ui.label("相关的成功、失败统计也会同步清除；后续执行会生成新日志。");
                    }
                }
                Confirm::Quit => {
                    ui.heading("退出 CronDesk");
                    ui.label(format!(
                        "退出后停止自动调度，并停止 {} 项正在执行的任务和 {} 个活动服务。",
                        self.snapshot.running,
                        self.snapshot
                            .services
                            .iter()
                            .filter(|s| s.pid.is_some() || s.status == "restarting")
                            .count()
                    ));
                }
            }
            ui.horizontal(|ui| {
                cancel = ui.button("取消").clicked();
                accept = ui.button(RichText::new("确定").color(RED)).clicked();
            });
        });
        if accept {
            match self.confirm.take().unwrap() {
                Confirm::Delete(id, _) => self.engine.send(Action::Delete(id)),
                Confirm::DeleteService(id, _) => {
                    self.engine.send(Action::Service(ServiceAction::Delete(id)))
                }
                Confirm::ClearLogs(service, owner, _) => {
                    self.engine.send(Action::ClearLogs(service, owner))
                }
                Confirm::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        if cancel {
            self.confirm = None;
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.receive();
        while let Ok(action) = self.tray_events.try_recv() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            if matches!(action, TrayAction::Quit) {
                self.confirm = Some(Confirm::Quit);
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.tray.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else {
                self.confirm = Some(Confirm::Quit);
            }
        }
        egui::TopBottomPanel::top("navigation")
            .frame(egui::Frame::new().fill(Color32::WHITE).inner_margin(12))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(((ui.available_width() - 266.0) / 2.0).max(0.0));
                    for (service, label) in [(false, "定时任务"), (true, "服务")] {
                        if ui
                            .add_sized(
                                [128.0, 36.0],
                                egui::Button::new(label).selected(self.service_tab == service),
                            )
                            .clicked()
                        {
                            self.service_tab = service;
                            self.notice = None;
                        }
                    }
                });
            });
        egui::TopBottomPanel::bottom("footer")
            .frame(egui::Frame::new().fill(Color32::WHITE).inner_margin(12))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(if self.scheduler_alive {
                            "● 调度器运行中"
                        } else {
                            "● 调度器已停止，请重启应用"
                        })
                        .color(if self.scheduler_alive { GREEN } else { RED })
                        .size(12.0),
                    );
                    ui.label(
                        RichText::new(if self.tray.is_some() {
                            "关闭窗口后在托盘继续运行"
                        } else {
                            "请保持窗口开启"
                        })
                        .color(MUTED)
                        .size(12.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("退出").clicked() {
                            self.confirm = Some(Confirm::Quit);
                        }
                        ui.label(
                            RichText::new(Local::now().format("%Y-%m-%d %H:%M:%S %:z").to_string())
                                .size(11.0)
                                .color(MUTED),
                        )
                        .on_hover_text(format!(
                            "数据库：{}",
                            self.data_dir.join("crondesk.sqlite").display()
                        ));
                    });
                });
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(246, 248, 247))
                    .inner_margin(28),
            )
            .show(ctx, |ui| {
                self.header(ui);
                if self.service_tab {
                    self.service_list(ui);
                } else {
                    self.list(ui);
                }
                if let Some((text, error, _)) = &self.notice {
                    ui.colored_label(if *error { RED } else { GREEN }, text);
                }
            });
        if self
            .notice
            .as_ref()
            .is_some_and(|(_, error, time)| !*error && time.elapsed() > Duration::from_secs(6))
        {
            self.notice = None;
        }
        self.logs(ctx);
        self.editor(ctx);
        self.service_editor(ctx);
        self.confirmation(ctx);
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

fn primary(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).color(Color32::WHITE))
        .fill(GREEN)
        .min_size(Vec2::new(116.0, 38.0))
}

fn stat(ui: &mut egui::Ui, label: &str, count: i64, color: Color32) {
    ui.allocate_ui_with_layout(
        Vec2::new(96.0, 50.0),
        egui::Layout::top_down(Align::Center),
        |ui| {
            ui.label(
                RichText::new(count.to_string())
                    .size(27.0)
                    .strong()
                    .color(color),
            );
            ui.label(RichText::new(label).size(11.0).color(MUTED));
        },
    );
}

fn status(ui: &mut egui::Ui, value: &str) {
    let (text, color) = match value {
        "success" => ("成功", GREEN),
        "failed" => ("失败", RED),
        "running" => ("执行中", Color32::from_rgb(52, 113, 181)),
        "interrupted" => ("已中断", RED),
        "skipped" => ("已跳过", MUTED),
        "stopped" => ("已停止", MUTED),
        "exited" => ("正常退出", GREEN),
        "stopping" => ("停止中", MUTED),
        "restarting" => ("等待重试", Color32::from_rgb(170, 110, 30)),
        _ => ("未执行", MUTED),
    };
    ui.label(RichText::new(text).size(12.0).color(color));
}

fn format_time(value: &str) -> String {
    DateTime::parse_from_rfc3339(value)
        .map(|d| d.with_timezone(&Local).format("%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|_| value.into())
}

pub fn app_icon() -> egui::IconData {
    let mut rgba = Vec::with_capacity(32 * 32 * 4);
    for y in 0..32_i32 {
        for x in 0..32_i32 {
            let distance = (x - 16).pow(2) + (y - 16).pow(2);
            let white = (85..=120).contains(&distance)
                || ((15..=17).contains(&x) && (8..=17).contains(&y))
                || ((16..=22).contains(&x) && (15..=17).contains(&y));
            rgba.extend_from_slice(if white {
                &[255, 255, 255, 255]
            } else {
                &[21, 121, 94, 255]
            });
        }
    }
    egui::IconData {
        rgba,
        width: 32,
        height: 32,
    }
}

#[cfg(windows)]
fn restore_tray_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsIconic, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow,
    };
    // Hidden Windows windows do not repaint. Restore natively before waiting for update().
    // Tray callbacks run on the UI thread, and the main window outlives the tray.
    unsafe {
        let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
        ShowWindow(
            hwnd,
            if IsIconic(hwnd) != 0 {
                SW_RESTORE
            } else {
                SW_SHOW
            },
        );
        SetForegroundWindow(hwnd);
    }
}

fn create_tray(
    cc: &eframe::CreationContext<'_>,
    tx: mpsc::Sender<TrayAction>,
) -> anyhow::Result<TrayIcon> {
    #[cfg(windows)]
    let hwnd = {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let RawWindowHandle::Win32(handle) = cc.window_handle()?.as_raw() else {
            anyhow::bail!("无法获取 Windows 主窗口句柄");
        };
        handle.hwnd.get()
    };
    let context = cc.egui_ctx.clone();
    let show_window = move || {
        #[cfg(windows)]
        restore_tray_window(hwnd);
        context.request_repaint();
    };
    #[cfg(windows)]
    {
        let tx = tx.clone();
        let show_window = show_window.clone();
        tray_icon::TrayIconEvent::set_event_handler(Some(move |event| {
            if matches!(
                event,
                tray_icon::TrayIconEvent::DoubleClick {
                    button: tray_icon::MouseButton::Left,
                    ..
                }
            ) {
                let _ = tx.send(TrayAction::Show);
                show_window();
            }
        }));
    }
    let menu = Menu::new();
    let show = MenuItem::new("打开界面", true, None);
    let quit = MenuItem::new("退出 CronDesk", true, None);
    menu.append(&show)?;
    menu.append(&quit)?;
    let show_id = show.id().clone();
    let quit_id = quit.id().clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if event.id == show_id {
            let _ = tx.send(TrayAction::Show);
        } else if event.id == quit_id {
            let _ = tx.send(TrayAction::Quit);
        } else {
            return;
        }
        show_window();
    }));
    let mut icon = app_icon();
    if cfg!(target_os = "macos") {
        // Menu bar templates use only the clock silhouette, with no colored tile.
        for pixel in icon.rgba.as_chunks_mut::<4>().0 {
            if pixel[0] != 255 {
                pixel[3] = 0;
            }
        }
    }
    Ok(TrayIconBuilder::new()
        .with_tooltip("CronDesk · 定时任务与服务")
        .with_menu(Box::new(menu))
        .with_icon(tray_icon::Icon::from_rgba(icon.rgba, 32, 32)?)
        .with_icon_as_template(cfg!(target_os = "macos"))
        .with_menu_on_left_click(!cfg!(windows))
        .build()?)
}

#[cfg(all(test, windows))]
#[test]
fn tray_restores_native_window_without_a_ui_update() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, IsIconic, IsWindowVisible, IsZoomed, SW_HIDE, SW_MAXIMIZE,
        SW_MINIMIZE, ShowWindow, WS_OVERLAPPEDWINDOW,
    };
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    unsafe {
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            320,
            240,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        );
        assert!(!hwnd.is_null());
        assert_eq!(IsWindowVisible(hwnd), 0);
        // No egui update or repaint runs between hiding and restoring the window.
        for _ in 0..2 {
            restore_tray_window(hwnd as isize);
            assert_ne!(IsWindowVisible(hwnd), 0);
            assert_eq!(IsIconic(hwnd), 0);
            ShowWindow(hwnd, SW_MINIMIZE);
            assert_ne!(IsIconic(hwnd), 0);
            restore_tray_window(hwnd as isize);
            assert_ne!(IsWindowVisible(hwnd), 0);
            assert_eq!(IsIconic(hwnd), 0);
            ShowWindow(hwnd, SW_MAXIMIZE);
            ShowWindow(hwnd, SW_HIDE);
            restore_tray_window(hwnd as isize);
            assert_ne!(IsWindowVisible(hwnd), 0);
            assert_ne!(IsZoomed(hwnd), 0);
            ShowWindow(hwnd, SW_HIDE);
        }
        DestroyWindow(hwnd);
    }
}
