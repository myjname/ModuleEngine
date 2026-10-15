//! Editor presentation of shared host activity.
use eframe::egui::{self, Color32, RichText, Sense};
pub use module_engine_host::activity::{ActivitySnapshot, ActivityState, BuildProgress};
use std::time::Duration;
/// One compact summary prioritizes failures, then running work. Expanding it
/// reveals every result, so several ready subsystems do not consume viewport space.
/// Returns true while any activity is running, and requests animation repaints.
pub fn draw_activity(ui: &mut egui::Ui, activities: &[ActivitySnapshot]) -> bool {
    if activities.is_empty() {
        return false;
    }
    let running_count = activities
        .iter()
        .filter(|activity| activity.state == ActivityState::Running)
        .count();
    let failure = activities
        .iter()
        .find(|activity| activity.state == ActivityState::Failed);
    let active = activities
        .iter()
        .find(|activity| activity.state == ActivityState::Running);
    let activity = failure
        .or(active)
        .unwrap_or_else(|| activities.last().unwrap());
    let color = activity_color(ui, activity.state);
    let elapsed = format!("{:.1} с", activity.elapsed.as_secs_f64());
    let summary = match activity.state {
        ActivityState::Running => format!(
            "В работе: {running_count} · {elapsed} · {} — {}",
            activity.title,
            first_line(&activity.detail),
        ),
        ActivityState::Succeeded => format!(
            "Готово · {elapsed} · {} — {} · результатов: {}",
            activity.title,
            first_line(&activity.detail),
            activities.len(),
        ),
        ActivityState::Failed => {
            let extra = if running_count > 0 {
                format!(" · в работе: {running_count}")
            } else {
                String::new()
            };
            format!(
                "Ошибка{extra} · {elapsed} · {} — {}",
                first_line(activity.error.as_deref().unwrap_or(&activity.detail)),
                activity.title,
            )
        }
    };
    let id = ui.make_persistent_id("build-activities-details");
    let state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
    let mut toggle = false;
    let mut header = state.show_header(ui, |ui| {
        if running_count > 0 {
            ui.add(egui::Spinner::new().size(14.0));
        }
        let response = ui.add(
            egui::Label::new(RichText::new(summary).color(color).size(12.0))
                .truncate()
                .sense(Sense::click()),
        );
        toggle = response.clicked();
        response.on_hover_text("Открыть все операции и сведения об ошибках");
    });
    if toggle {
        header.toggle();
    }
    header.body(|ui| {
        egui::ScrollArea::vertical()
            .id_salt("activity-details")
            .max_height(220.0)
            .show(ui, |ui| {
                for (index, activity) in activities.iter().enumerate() {
                    ui.push_id((index, &activity.title), |ui| {
                        if index > 0 {
                            ui.separator();
                        }
                        let color = activity_color(ui, activity.state);
                        let elapsed = format!("{:.1} с", activity.elapsed.as_secs_f64());
                        ui.add(egui::Label::new(RichText::new(&activity.title).strong()).wrap());
                        let state = match activity.state {
                            ActivityState::Running => format!("Выполняется · прошло {elapsed}"),
                            ActivityState::Succeeded => format!("Завершено за {elapsed}"),
                            ActivityState::Failed => {
                                format!("Операция завершилась ошибкой за {elapsed}")
                            }
                        };
                        ui.add(egui::Label::new(RichText::new(state).color(color)).wrap());
                        ui.add(egui::Label::new(&activity.detail).selectable(true).wrap());
                        if let Some(error) = &activity.error {
                            ui.add(
                                egui::Label::new(RichText::new(error).color(color))
                                    .selectable(true)
                                    .wrap(),
                            );
                            if ui.small_button("Копировать ошибку").clicked() {
                                ui.ctx().copy_text(error.clone());
                            }
                        }
                    });
                }
            });
    });
    if running_count > 0 {
        ui.ctx().request_repaint_after(Duration::from_millis(33));
    }
    running_count > 0
}

fn activity_color(ui: &egui::Ui, state: ActivityState) -> Color32 {
    match state {
        ActivityState::Running => ui.visuals().text_color(),
        ActivityState::Succeeded => Color32::from_rgb(128, 207, 161),
        ActivityState::Failed => Color32::from_rgb(246, 134, 130),
    }
}

fn first_line(text: &str) -> &str {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_clone_reports_stage_and_terminal_elapsed_stays_frozen() {
        let progress = BuildProgress::new("Рендер Scene");
        let worker = progress.clone();
        std::thread::spawn(move || {
            worker.stage("Подготовка GPU-проходов");
            worker.succeed("Конфигурация применена");
        })
        .join()
        .unwrap();
        let snapshot = progress.snapshot();
        assert_eq!(snapshot.state, ActivityState::Succeeded);
        assert_eq!(snapshot.detail, "Конфигурация применена");
        assert!(snapshot.error.is_none());
        std::thread::sleep(Duration::from_millis(1));
        assert_eq!(progress.snapshot().elapsed, snapshot.elapsed);
        progress.stage("Запоздалое сообщение");
        assert_eq!(progress.snapshot().state, ActivityState::Succeeded);
        assert_eq!(progress.snapshot().detail, "Конфигурация применена");
    }

    #[test]
    fn acceptance_failure_is_visible_and_cannot_be_hidden_by_late_success() {
        let progress = BuildProgress::new("Rust controller");
        progress.stage("Проверка состояния");
        progress.succeed("Скомпилировано");
        progress.fail("Схема состояния несовместима; нужна миграция");
        progress.succeed("Запоздалый результат");
        let snapshot = progress.snapshot();
        assert_eq!(snapshot.state, ActivityState::Failed);
        assert_eq!(
            snapshot.error.as_deref(),
            Some("Схема состояния несовместима; нужна миграция")
        );
        progress.fail("Дополнительные сведения об ошибке применения");
        assert_eq!(progress.snapshot().elapsed, snapshot.elapsed);
    }

    #[test]
    fn ui_shows_pending_success_and_failure_and_expands_full_error() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        let pending = BuildProgress::new("Scene");
        pending.stage("Подготовка GPU");
        let done = BuildProgress::new("Rust");
        done.succeed("Код применён");
        let failed = BuildProgress::new("Game");
        failed.stage("Компиляция HLSL");
        failed.fail("DXC: неверный тип\nshader.hlsl:42:7: expected float4");
        let snapshots = [pending.snapshot(), done.snapshot(), failed.snapshot()];
        let input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(500.0, 400.0),
            )),
            events,
            ..Default::default()
        };
        let output = ctx.run_ui(input(vec![]), |ui| assert!(draw_activity(ui, &snapshots)));
        let labels: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    Some(text.galley.text())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            labels.len(),
            1,
            "several subsystems share one collapsed summary"
        );
        assert!(labels.iter().any(|text| text.contains("Ошибка")
            && text.contains("DXC")
            && text.contains("в работе: 1")));
        assert!(labels.iter().all(|text| !text.contains('%')));
        let click = output
            .shapes
            .iter()
            .find_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape
                    && text.galley.text().starts_with("Ошибка")
                {
                    Some(text.pos + text.galley.size() * 0.5)
                } else {
                    None
                }
            })
            .unwrap();
        output.drop_without_applying_deltas();
        let output = ctx.run_ui(
            input(vec![
                egui::Event::PointerMoved(click),
                egui::Event::PointerButton {
                    pos: click,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: click,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]),
            |ui| {
                draw_activity(ui, &snapshots);
            },
        );
        assert!(output.shapes.iter().any(|shape| {
            if let egui::Shape::Text(text) = &shape.shape {
                text.galley.text().contains("shader.hlsl:42:7")
            } else {
                false
            }
        }));
        assert!(output.shapes.iter().any(|shape| {
            if let egui::Shape::Text(text) = &shape.shape {
                text.galley.text().contains("Подготовка GPU")
            } else {
                false
            }
        }));
        assert!(output.shapes.iter().any(|shape| {
            if let egui::Shape::Text(text) = &shape.shape {
                text.galley.text().contains("Код применён")
            } else {
                false
            }
        }));
        output.drop_without_applying_deltas();
    }

    #[test]
    fn long_activity_summary_fits_a_narrow_panel_and_idle_returns_false() {
        let ctx = egui::Context::default();
        let failed = BuildProgress::new("Длинное название операции ".repeat(20));
        failed.fail("Длинное сообщение компилятора ".repeat(100));
        let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(210.0, 150.0));
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(bounds),
                ..Default::default()
            },
            |ui| {
                assert!(!draw_activity(ui, &[failed.snapshot()]));
                assert!(ui.min_rect().right() <= bounds.right() + 0.01);
                assert!(!draw_activity(ui, &[]));
            },
        );
        output.drop_without_applying_deltas();
    }
    #[test]
    fn many_successful_subsystems_use_one_row_and_running_shows_its_current_stage() {
        let ctx = egui::Context::default();
        let mut activities: Vec<_> = (0..20)
            .map(|index| {
                let progress = BuildProgress::new(format!("Рендер {index}"));
                progress.succeed("Конфигурация применена");
                progress.snapshot()
            })
            .collect();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(900.0, 300.0),
            )),
            ..Default::default()
        };
        let output = ctx.run_ui(input(), |ui| {
            assert!(!draw_activity(ui, &activities));
            assert!(
                ui.min_rect().height() < 35.0,
                "ready job count must not take viewport height"
            );
        });
        assert_eq!(
            output
                .shapes
                .iter()
                .filter(|shape| matches!(shape.shape, egui::Shape::Text(_)))
                .count(),
            1
        );
        output.drop_without_applying_deltas();
        let running = BuildProgress::new("Scene");
        running.stage("Создание рендера");
        activities.push(running.snapshot());
        let output = ctx.run_ui(input(), |ui| assert!(draw_activity(ui, &activities)));
        assert!(output.shapes.iter().any(|shape| {
            if let egui::Shape::Text(text) = &shape.shape {
                text.galley.text().contains("Создание рендера")
                    && text.galley.text().contains("В работе: 1")
            } else {
                false
            }
        }));
        output.drop_without_applying_deltas();
    }
}
