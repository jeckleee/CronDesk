use super::*;

impl App {
    pub(super) fn service_list(&mut self, ui: &mut egui::Ui) {
        ui.add_space(26.0);
        ui.horizontal(|ui| {
            ui.heading("服务列表");
            ui.label(RichText::new("持续运行 · 进程托管").size(12.0).color(GREEN));
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if ui.button("服务日志").clicked() {
                    self.open_log_view(true, None, "全部服务日志".into());
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.service_search)
                        .hint_text("搜索服务名称或命令")
                        .desired_width(220.0),
                );
            });
        });
        ui.add_space(12.0);
        ui.label(
            RichText::new(
                "运行中代表进程存活；请使用前台命令，例如 node aa.js，不要自行转入后台。",
            )
            .size(12.0)
            .color(MUTED),
        );
        ui.add_space(14.0);
        let search = self.service_search.to_lowercase();
        let services: Vec<_> = self
            .snapshot
            .services
            .iter()
            .filter(|s| {
                s.name.to_lowercase().contains(&search)
                    || s.command.to_lowercase().contains(&search)
            })
            .cloned()
            .collect();
        egui::Frame::new()
            .fill(Color32::WHITE)
            .stroke(Stroke::new(1.0_f32, BORDER))
            .corner_radius(10)
            .inner_margin(16)
            .show(ui, |ui| {
                if services.is_empty() {
                    ui.set_min_height(280.0);
                    ui.vertical_centered(|ui| {
                        ui.add_space(65.0);
                        ui.label(
                            RichText::new(if self.snapshot.services.is_empty() {
                                "让你的服务持续运行"
                            } else {
                                "没有匹配的服务"
                            })
                            .size(21.0)
                            .strong(),
                        );
                        ui.label(
                            RichText::new("设置启动命令与执行目录，在这里启动、停止并查看日志。")
                                .color(MUTED),
                        );
                        ui.add_space(16.0);
                        if self.snapshot.services.is_empty()
                            && ui.add(primary("＋ 创建第一个服务")).clicked()
                        {
                            self.notice = None;
                            self.service_editor = Some(Service::default());
                        }
                    });
                    return;
                }
                let height = (ui.available_height() - 80.0).max(160.0);
                TableBuilder::new(ui)
                    .striped(true)
                    .cell_layout(egui::Layout::left_to_right(Align::Center))
                    .column(Column::remainder().at_least(200.0))
                    .column(Column::exact(90.0))
                    .column(Column::exact(140.0))
                    .column(Column::exact(265.0))
                    .max_scroll_height(height)
                    .header(32.0, |mut row| {
                        for title in ["服务 / 命令 / 执行目录", "状态", "进程 / 运行时长", "操作"]
                        {
                            row.col(|ui| {
                                ui.label(RichText::new(title).size(12.0).color(MUTED));
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(104.0, services.len(), |mut row| {
                            let service = &services[row.index()];
                            let active = service.pid.is_some() || service.status == "restarting";
                            row.col(|ui| {
                                ui.vertical(|ui| {
                                    ui.add_space(8.0);
                                    ui.add(
                                        egui::Label::new(RichText::new(&service.name).strong())
                                            .truncate(),
                                    )
                                    .on_hover_text(&service.name);
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(
                                                service.command.lines().next().unwrap_or(""),
                                            )
                                            .monospace()
                                            .size(11.0)
                                            .color(MUTED),
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(&service.command);
                                    let dir = if service.cwd.is_empty() {
                                        self.default_dir.to_string_lossy().into_owned()
                                    } else {
                                        service.cwd.clone()
                                    };
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(format!("目录：{dir}"))
                                                .size(11.0)
                                                .color(MUTED),
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(dir);
                                });
                            });
                            row.col(|ui| {
                                ui.vertical(|ui| {
                                    if service.status == "running" {
                                        ui.colored_label(GREEN, "运行中");
                                    } else {
                                        status(ui, &service.status);
                                    }
                                    ui.label(
                                        RichText::new(format!("重试 {} 次", service.restarts))
                                            .size(11.0)
                                            .color(MUTED),
                                    );
                                })
                                .response
                                .on_hover_text(format!(
                                    "最近启动：{}\n退出码：{}\n随工具启动：{}",
                                    format_time(&service.started),
                                    service
                                        .exit_code
                                        .map(|c| c.to_string())
                                        .unwrap_or_else(|| "—".into()),
                                    if service.autostart { "是" } else { "否" }
                                ));
                            });
                            row.col(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(
                                        service
                                            .pid
                                            .map(|p| format!("PID {p}"))
                                            .unwrap_or_else(|| "—".into()),
                                    );
                                    let seconds = service.uptime;
                                    ui.label(
                                        RichText::new(if service.pid.is_some() {
                                            format!(
                                                "{:02}:{:02}:{:02}",
                                                seconds / 3600,
                                                seconds / 60 % 60,
                                                seconds % 60
                                            )
                                        } else {
                                            "未运行".into()
                                        })
                                        .size(12.0)
                                        .color(MUTED),
                                    );
                                });
                            });
                            row.col(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                ui.spacing_mut().button_padding = Vec2::new(7.0, 6.0);
                                if ui.button(if active { "停止" } else { "启动" }).clicked() {
                                    self.engine.send(Action::Service(if active {
                                        ServiceAction::Stop(service.id)
                                    } else {
                                        ServiceAction::Start(service.id)
                                    }));
                                }
                                if ui
                                    .add_enabled(
                                        service.status != "stopping",
                                        egui::Button::new("重启"),
                                    )
                                    .clicked()
                                {
                                    self.engine
                                        .send(Action::Service(ServiceAction::Restart(service.id)));
                                }
                                if ui.button("日志").clicked() {
                                    self.open_log_view(
                                        true,
                                        Some(service.id),
                                        service.name.clone(),
                                    );
                                }
                                ui.menu_button("更多", |ui| {
                                    if ui
                                        .add_enabled(!active, egui::Button::new("编辑服务"))
                                        .clicked()
                                    {
                                        self.notice = None;
                                        self.service_editor = Some(service.clone());
                                        ui.close();
                                    }
                                    if ui
                                        .add_enabled(
                                            !active,
                                            egui::Button::new(RichText::new("删除服务").color(RED)),
                                        )
                                        .clicked()
                                    {
                                        self.confirm = Some(Confirm::DeleteService(
                                            service.id,
                                            service.name.clone(),
                                        ));
                                        ui.close();
                                    }
                                });
                            });
                        })
                    });
            });
        ui.add_space(12.0);
        ui.label(RichText::new("关闭窗口后服务继续运行 · 退出工具时停止服务 · 停止时先尝试正常退出，5 秒后强制结束进程树").size(12.0).color(MUTED));
    }

    pub(super) fn service_editor(&mut self, ctx: &egui::Context) {
        let Some(service) = &mut self.service_editor else {
            return;
        };
        let mut cancel = false;
        let mut save = false;
        egui::Modal::new(egui::Id::new("service-editor")).show(ctx,|ui| {
            ui.set_width(600.0);
            ui.heading(if service.id==0 {"新建服务"}else{"编辑服务"});
            ui.label(RichText::new("保存配置后，点击“启动”开始运行。修改配置需先停止服务。").color(MUTED));
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height()-190.0).max(230.0)).show(ui,|ui| {
                ui.label("服务名称");
                ui.add(egui::TextEdit::singleline(&mut service.name).id_salt("service-name").hint_text("例如：Node API 服务").desired_width(f32::INFINITY));
                ui.label("启动命令");
                ui.add(egui::TextEdit::multiline(&mut service.command).id_salt("service-command").desired_rows(3)
                    .font(egui::TextStyle::Monospace).hint_text("node aa.js").desired_width(f32::INFINITY));
                ui.label("服务执行目录（可选）");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut service.cwd).id_salt("service-cwd").hint_text("选择脚本或项目所在文件夹").desired_width(480.0));
                    if ui.button("选择…").clicked() && let Some(path)=rfd::FileDialog::new().set_title("选择服务执行目录").pick_folder() {service.cwd=path.to_string_lossy().into_owned();}
                });
                ui.label(RichText::new(format!("留空使用：{}",self.default_dir.display())).size(11.0).color(MUTED));
                ui.label("执行环境");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut service.powershell,false,if cfg!(windows) {"默认（cmd.exe）"}else{"默认（/bin/sh）"});
                    ui.selectable_value(&mut service.powershell,true,"PowerShell 7（pwsh）");
                });
                if service.powershell {
                    ui.checkbox(&mut service.load_profile,"加载 PowerShell profile（读取其中设置的环境变量）");
                    ui.label(RichText::new("需已安装 PowerShell 7 并将 pwsh 加入 PATH；不显示或保存环境变量内容。").size(11.0).color(MUTED));
                }
                ui.separator();
                ui.checkbox(&mut service.autostart,"打开 CronDesk 时自动启动此服务");
                ui.checkbox(&mut service.restart_on_failure,"异常退出后自动重试（手动停止后不会重启）");
                if service.restart_on_failure {
                    ui.horizontal(|ui| {
                        ui.label("重试间隔"); ui.add(egui::DragValue::new(&mut service.restart_delay).range(1..=3600).suffix(" 秒"));
                        ui.label("次数上限"); ui.add(egui::DragValue::new(&mut service.max_restarts).range(1..=100).suffix(" 次"));
                    });
                    ui.label(RichText::new("上限按本次启动累计，手动启动或重启后重置；正常退出不自动重试。").size(11.0).color(MUTED));
                }
                if let Some((text,true,_))=&self.notice {ui.colored_label(RED,text);}
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new("服务以当前用户权限运行。").size(11.0).color(MUTED));
                ui.with_layout(egui::Layout::right_to_left(Align::Center),|ui| {
                    save=ui.add_enabled(!self.saving,primary(if self.saving {"保存中…"}else{"保存服务"})).clicked();
                    cancel=ui.add_enabled(!self.saving,egui::Button::new("取消")).clicked();
                });
            });
        });
        if save {
            match crate::services::validate(service) {
                Ok(()) => {
                    self.engine
                        .send(Action::Service(ServiceAction::Save(service.clone())));
                    self.saving = true;
                }
                Err(e) => self.tell(&format!("{e:#}"), true),
            }
        }
        if cancel {
            self.service_editor = None;
        }
    }
}
