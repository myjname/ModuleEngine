use super::*;

impl Editor {
    pub(super) fn gpu_pass_controls(&mut self, ui: &mut egui::Ui, node: &module_engine_core::Node) {
        let mut params = node.params.clone();
        let mut changed = false;
        if node.module == "gpu.tonemap" {
            let mut mode = params
                .get("mode")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("aces")
                .to_owned();
            let old = mode.clone();
            egui::ComboBox::from_id_salt("tone-mode")
                .selected_text(&mode)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut mode, "aces".into(), "ACES");
                    ui.selectable_value(&mut mode, "reinhard".into(), "Reinhard");
                });
            if mode != old {
                params["mode"] = serde_json::Value::String(mode);
                changed = true;
            }
        }
        let manifest = if node.module == "gpu.custom" {
            params.get("manifest").and_then(|v| {
                serde_json::from_value::<module_engine_vulkan::PassManifest>(v.clone()).ok()
            })
        } else {
            module_engine_vulkan::builtin_manifest(&node.module)
        };
        if let Some(manifest) = &manifest {
            for name in &manifest.parameters {
                if name == "mode" && node.module != "gpu.custom" {
                    continue;
                }
                let mut value = params
                    .get(name)
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(1.0);
                if ui
                    .add(
                        egui::DragValue::new(&mut value)
                            .speed(0.02)
                            .prefix(format!("{name} = ")),
                    )
                    .changed()
                {
                    params[name] = serde_json::json!(value);
                    changed = true;
                }
            }
            if node.module == "gpu.ssao" || node.module == "gpu.fxaa" {
                let mut enabled = params
                    .get("enabled")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true);
                if ui.checkbox(&mut enabled, "Проход включён").changed() {
                    params["enabled"] = serde_json::json!(enabled);
                    changed = true;
                }
            }
            egui::CollapsingHeader::new("Контракты GPU-ресурсов").show(ui, |ui| {
                for spec in manifest.inputs.iter().chain(&manifest.outputs) {
                    ui.label(
                        RichText::new(format!(
                            "{} · {} · {} · {} · samples {}",
                            spec.name, spec.semantic, spec.format, spec.access, spec.samples
                        ))
                        .size(10.0),
                    );
                }
            });
        }
        if changed {
            if let Some(target) = self.graph_mut().nodes.iter_mut().find(|n| n.id == node.id) {
                target.params = params;
            }
            self.inspector_id = None;
            self.changed(true);
        }
        if node.module == "gpu.custom" {
            if ui.button("Редактировать HLSL").clicked() {
                self.current_shader = node
                    .params
                    .get("source_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .into();
                self.tab = Tab::Shader;
            }
            ui.horizontal(|ui| {
                if ui.button("Новый Compute-шаблон").clicked() {
                    self.set_custom_shader_template(&node.id, false);
                }
                if ui.button("Новый Scene-шаблон").clicked() {
                    self.set_custom_shader_template(&node.id, true);
                }
            });
        } else if ui.button("Заменить на редактируемый Custom HLSL").clicked()
            && let (Some(source), Some(mut manifest)) = (
                module_engine_vulkan::builtin_hlsl_source(&node.module),
                module_engine_vulkan::builtin_manifest(&node.module),
            )
        {
            if manifest.stage == "raster" {
                manifest.vertex_entry = Some("mainVS".into());
            }
            let key = format!(
                "{}_editable_{}",
                node.id,
                self.document.shader_sources.len()
            );
            self.document
                .shader_sources
                .insert(key.clone(), source.into());
            if let Some(target) = self.graph_mut().nodes.iter_mut().find(|n| n.id == node.id) {
                target.module = "gpu.custom".into();
                target.params["source_id"] = serde_json::json!(key);
                target.params["manifest"] = serde_json::to_value(manifest).unwrap();
            }
            self.current_shader = key;
            self.tab = Tab::Shader;
            self.inspector_id = None;
            self.changed(true);
        }
    }

    fn set_custom_shader_template(&mut self, id: &str, raster: bool) {
        let key = format!(
            "{id}_{}_{}",
            if raster { "scene" } else { "compute" },
            self.document.shader_sources.len()
        );
        let (manifest, source) = if raster {
            (
                module_engine_vulkan::custom_raster_manifest(),
                module_engine_vulkan::demo_custom_raster_source(),
            )
        } else {
            (
                module_engine_vulkan::custom_compute_manifest(),
                module_engine_vulkan::demo_custom_compute_source(),
            )
        };
        self.document.shader_sources.insert(key.clone(), source);
        if let Some(node) = self.graph_mut().nodes.iter_mut().find(|n| n.id == id) {
            node.params = serde_json::json!({"source_id":key,"manifest":manifest});
            node.params[if raster { "shift" } else { "gain" }] =
                serde_json::json!(if raster { 0.0 } else { 1.0 });
        }
        self.current_shader = key;
        self.tab = Tab::Shader;
        self.inspector_id = None;
        self.changed(true);
    }

    pub(super) fn shader_panel(&mut self, ui: &mut egui::Ui) {
        if !self
            .document
            .shader_sources
            .contains_key(&self.current_shader)
        {
            self.current_shader = self
                .document
                .shader_sources
                .keys()
                .next()
                .cloned()
                .unwrap_or_default();
        }
        ui.horizontal(|ui| {
            ui.heading("Custom / HLSL");
            egui::ComboBox::from_id_salt("shader-choice")
                .selected_text(&self.current_shader)
                .show_ui(ui, |ui| {
                    for key in self.document.shader_sources.keys() {
                        ui.selectable_value(&mut self.current_shader, key.clone(), key);
                    }
                });
            if ui.button("Применить HLSL").clicked() {
                self.rebuild();
            }
            if ui.button("Открыть граф Vulkan").clicked() {
                self.graph_path.clear();
                if let Some(id) = self
                    .document
                    .root
                    .nodes
                    .iter()
                    .find(|n| n.module == "render.vulkan")
                    .map(|n| n.id.clone())
                {
                    self.engine_actions(vec![CanvasAction::Open(id)]);
                    self.tab = Tab::Graph;
                }
            }
        });
        ui.label("Редактируется обычный HLSL. Ресурсы, стадии и точки входа объявлены в manifest выбранного узла. Ошибка компиляции сохраняет предыдущий рабочий рендер.");
        if self.preview_builder.is_pending() {
            ui.label("Компиляция GPU-проходов в фоне…");
        }
        if let Some(error) = &self.preview_builder.error {
            egui::CollapsingHeader::new("Диагностика GPU-кандидата")
                .default_open(true)
                .show(ui, |ui| {
                    ui.colored_label(Color32::from_rgb(246, 134, 130), error);
                });
        }
        let Some(source) = self.document.shader_sources.get_mut(&self.current_shader) else {
            ui.label("Раскройте Vulkan renderer и добавьте Custom HLSL или замените выбранный встроенный проход на Custom.");
            return;
        };
        if ui
            .add_sized(
                ui.available_size(),
                egui::TextEdit::multiline(source)
                    .font(egui::TextStyle::Monospace)
                    .code_editor()
                    .desired_width(f32::INFINITY),
            )
            .changed()
        {
            self.changed(false);
            self.status = "HLSL изменён; нажмите «Применить HLSL» для компиляции".into();
        }
    }

    pub(super) fn command_inspector(&mut self, ui: &mut egui::Ui) {
        let node = self
            .code_canvas
            .selected
            .as_ref()
            .and_then(|id| {
                self.command_program
                    .as_ref()?
                    .nodes()
                    .iter()
                    .find(|n| &n.id == id)
            })
            .cloned();
        let Some(node) = node else {
            ui.label("Выберите параметр команды: число, строку или флаг. Графическая правка изменит Rust и запустит компиляцию кандидата.");
            return;
        };
        ui.label(RichText::new(&node.label).strong());
        if self.source_error.is_some() {
            ui.label("Визуальные правки недоступны для текущего исходника");
            return;
        }
        let result = match node.kind {
            CommandNodeKind::Number { mut value } => ui
                .add(egui::DragValue::new(&mut value).speed(0.05))
                .changed()
                .then(|| {
                    self.command_program
                        .as_mut()
                        .unwrap()
                        .set_number(&node.id, value)
                }),
            CommandNodeKind::Text { mut value } => {
                ui.text_edit_singleline(&mut value).changed().then(|| {
                    self.command_program
                        .as_mut()
                        .unwrap()
                        .set_string(&node.id, &value)
                })
            }
            CommandNodeKind::Boolean { mut value } => {
                ui.checkbox(&mut value, "Значение").changed().then(|| {
                    self.command_program
                        .as_mut()
                        .unwrap()
                        .set_boolean(&node.id, value)
                })
            }
            CommandNodeKind::Opaque { source } => {
                ui.label(RichText::new(source).monospace());
                ui.label("Этот Rust-фрагмент редактируется текстом и исполняется компилятором");
                None
            }
            _ => {
                ui.label(
                    "Входы команды показывают её параметры. Порядок команд задаётся Rust-кодом.",
                );
                None
            }
        };
        if let Some(result) = result {
            match result {
                Ok(()) => {
                    self.sync_program_source(false);
                    self.status = "Параметр изменён в исходнике Rust".into();
                }
                Err(error) => self.status = error.to_string(),
            }
        }
    }

    pub(super) fn history_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("История проекта");
        ui.label("История относится к графу, исходникам, параметрам и раскладке. Состояние симуляции не записывается.");
        ui.horizontal(|ui| {
            if ui.button("Отменить").clicked() {
                self.undo();
            }
            if ui.button("Повторить").clicked() {
                self.redo();
            }
            ui.label(format!(
                "Шаг {} / {}",
                self.history.cursor(),
                self.history.entries().len().saturating_sub(1)
            ));
        });
        let home = PathBuf::from(&self.path)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();
        let store = CheckpointStore::new(home.join(".history"));
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.checkpoint_label);
            if ui.button("Сохранить контрольную версию").clicked() {
                self.flush_history();
                match store.save(&self.checkpoint_label, &self.document) {
                    Ok(saved) => self.status = format!("Версия {} сохранена", saved.id),
                    Err(error) => self.status = error,
                }
            }
        });
        egui::CollapsingHeader::new("Сохранённые версии").show(ui, |ui| match store.list() {
            Ok(checkpoints) => {
                for entry in checkpoints.iter().rev().take(30) {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} · {}", entry.id, entry.label));
                        if ui
                            .add_enabled(
                                !self.json_dirty && !self.game_workbench.has_graph_draft(),
                                egui::Button::new("Восстановить"),
                            )
                            .clicked()
                        {
                            if let Err(error) =
                                self.validate_asset_restore(&entry.document).and_then(|_| {
                                    self.validate_candidate(
                                        &entry.document,
                                        self.can_author_engine(),
                                    )
                                })
                            {
                                self.status = error;
                                return;
                            }
                            self.flush_history();
                            self.history
                                .record("Восстановление версии", &entry.document);
                            self.restore_document(entry.document.clone());
                        }
                    });
                }
            }
            Err(error) => {
                ui.colored_label(Color32::from_rgb(246, 134, 130), error);
            }
        });
        ui.separator();
        ui.heading("Сравнение и объединение файлов");
        ui.horizontal(|ui| {
            ui.label("Общая исходная версия");
            ui.text_edit_singleline(&mut self.merge_base);
        });
        ui.horizontal(|ui| {
            ui.label("Изменённый файл другого участника");
            ui.text_edit_singleline(&mut self.merge_theirs);
        });
        ui.horizontal(|ui| {
            if ui.button("Сравнить с текущим проектом").clicked() {
                match project::read(&PathBuf::from(&self.merge_base))
                    .and_then(|base| diff_documents(&base, &self.document))
                {
                    Ok(changes) => {
                        self.diff_text = changes
                            .iter()
                            .map(|c| format!("{} {}", c.kind, c.path))
                            .collect::<Vec<_>>()
                            .join("\n")
                    }
                    Err(error) => self.status = error,
                }
            }
            if ui.button("Подготовить объединение").clicked() {
                let result = project::read(&PathBuf::from(&self.merge_base)).and_then(|base| {
                    project::read(&PathBuf::from(&self.merge_theirs))
                        .and_then(|theirs| merge_documents(&base, &self.document, &theirs))
                });
                match result {
                    Ok(result) => {
                        self.status = format!(
                            "Конфликтов: {}. Исходные файлы не изменены.",
                            result.conflicts.len()
                        );
                        self.merge_result = Some(result);
                    }
                    Err(error) => self.status = error,
                }
            }
        });
        egui::ScrollArea::vertical().show(ui, |ui| {
            if !self.diff_text.is_empty() {
                ui.label(RichText::new(&self.diff_text).monospace());
            }
            let mut choice = None;
            if let Some(result) = &self.merge_result {
                for (index, conflict) in result.conflicts.iter().enumerate() {
                    egui::CollapsingHeader::new(format!("Конфликт {}", conflict.path))
                        .default_open(true)
                        .show(ui, |ui| {
                            ui.label(format!(
                                "База: {}",
                                conflict
                                    .base
                                    .as_ref()
                                    .map(ToString::to_string)
                                    .unwrap_or_else(|| "Отсутствует".into())
                            ));
                            ui.label(format!(
                                "Текущая: {}",
                                conflict
                                    .ours
                                    .as_ref()
                                    .map(ToString::to_string)
                                    .unwrap_or_else(|| "Отсутствует".into())
                            ));
                            ui.label(format!(
                                "Другая: {}",
                                conflict
                                    .theirs
                                    .as_ref()
                                    .map(ToString::to_string)
                                    .unwrap_or_else(|| "Отсутствует".into())
                            ));
                            ui.horizontal(|ui| {
                                if ui.button("Оставить текущую").clicked() {
                                    choice = Some((index, ConflictChoice::Ours));
                                }
                                if ui.button("Взять другую").clicked() {
                                    choice = Some((index, ConflictChoice::Theirs));
                                }
                                if ui.button("Вернуть базу").clicked() {
                                    choice = Some((index, ConflictChoice::Base));
                                }
                            });
                        });
                }
            }
            if let Some((index, choice)) = choice
                && let Some(result) = &mut self.merge_result
                && let Err(error) = result.resolve(index, choice)
            {
                self.status = error;
            }
            let resolved = self
                .merge_result
                .as_ref()
                .is_some_and(MergeResult::is_resolved);
            if ui
                .add_enabled(
                    resolved && !self.json_dirty && !self.game_workbench.has_graph_draft(),
                    egui::Button::new("Применить объединённый проект"),
                )
                .clicked()
                && let Some(result) = self.merge_result.take()
            {
                if let Err(error) = self.validate_asset_restore(&result.document).and_then(|_| {
                    self.validate_candidate(&result.document, self.can_author_engine())
                }) {
                    self.status = error;
                    self.merge_result = Some(result);
                    return;
                }
                self.flush_history();
                self.history
                    .record("Объединение проектов", &result.document);
                self.restore_document(result.document);
            }
            if ui
                .button("Сохранить результат вместе с конфликтами")
                .clicked()
                && let Some(result) = &self.merge_result
            {
                let path = home.join(format!(
                    "merge-{}.json",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis()
                ));
                self.status = match result.save(&path) {
                    Ok(()) => format!("Сохранено {}", path.display()),
                    Err(error) => error,
                };
            }
        });
    }
}
