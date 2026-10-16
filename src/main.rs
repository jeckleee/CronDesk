#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod engine;
mod services;
mod store;
mod ui;

use anyhow::{Context, Result};
use std::fs::{self, OpenOptions};

fn main() {
    if let Err(error) = start() {
        rfd::MessageDialog::new()
            .set_title("CronDesk")
            .set_description(format!("{error:#}"))
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
}

fn start() -> Result<()> {
    let data_dir = dirs::data_local_dir()
        .context("找不到用户数据目录")?
        .join("CronDesk");
    fs::create_dir_all(&data_dir)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data_dir.join("instance.lock"))?;
    lock.try_lock()
        .context("CronDesk 已在运行，请通过系统托盘打开已有窗口")?;
    let default_dir = dirs::home_dir().context("找不到用户主目录")?;
    let engine = engine::Engine::start(data_dir.clone(), default_dir.clone())?;
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1240.0, 780.0])
            .with_min_inner_size([1000.0, 640.0])
            .with_app_id("com.crondesk.desktop")
            .with_icon(ui::app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "CronDesk · 定时任务与服务",
        options,
        Box::new(move |cc| {
            Ok(Box::new(ui::App::new(
                cc,
                engine,
                lock,
                data_dir,
                default_dir,
            )))
        }),
    )
    .map_err(|e| anyhow::anyhow!("打开桌面窗口失败：{e}"))
}
