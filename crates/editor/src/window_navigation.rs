//! Window-menu navigation is personal UI state, independent of project history.
//! Reuse a matching area; otherwise split the last active area without replacing it.
use super::Editor;
use super::dock::{DockLayout, DockResponse, EditorDescriptor, EditorType, WorkspaceKind};
use eframe::egui;

#[derive(Default)]
pub(super) struct WindowNavigation {
    active: Option<u64>,
    recent: Vec<u64>,
}

struct OpenedWindow {
    id: u64,
    layout_changed: bool,
}

impl WindowNavigation {
    pub(super) fn active(&self) -> Option<u64> {
        self.active
    }

    fn reconcile(&mut self, layout: &DockLayout) {
        let panes = layout.panes();
        self.recent
            .retain(|id| panes.iter().any(|(pane, _)| pane == id));
        if !self
            .active
            .is_some_and(|id| panes.iter().any(|(pane, _)| *pane == id))
        {
            self.active = self
                .recent
                .last()
                .copied()
                .or_else(|| panes.first().map(|(id, _)| *id));
        }
    }

    fn activate(&mut self, id: u64, layout: &DockLayout) {
        if layout.panes().iter().any(|(pane, _)| *pane == id) {
            self.active = Some(id);
            self.recent.retain(|pane| *pane != id);
            self.recent.push(id);
        }
    }

    fn open(
        &mut self,
        layout: &mut DockLayout,
        editor: EditorType,
        available: &[EditorDescriptor],
    ) -> Result<OpenedWindow, String> {
        if !available.iter().any(|item| item.editor == editor) {
            return Err("This window is unavailable for the current modules/distribution".into());
        }
        let restricted = layout.restrict(available);
        self.reconcile(layout);
        let panes = layout.panes();
        let existing = self
            .recent
            .iter()
            .rev()
            .find(|id| {
                panes
                    .iter()
                    .any(|(pane, kind)| pane == *id && *kind == editor)
            })
            .copied()
            .or_else(|| {
                panes
                    .iter()
                    .find(|(_, kind)| *kind == editor)
                    .map(|(id, _)| *id)
            });
        let (id, created) = if let Some(id) = existing {
            (id, false)
        } else {
            let active = self.active.ok_or("Workspace has no active area")?;
            let added = layout.split(active, super::dock::Axis::Horizontal)?;
            layout.choose(added, editor);
            (added, true)
        };
        self.activate(id, layout);
        Ok(OpenedWindow {
            id,
            layout_changed: restricted || created,
        })
    }
}

impl Editor {
    /// Menu commands stay in the selected Engine/Project workspace. Availability
    /// comes from installed, enabled module panels and the distribution policy.
    pub(super) fn open_window(&mut self, editor: EditorType) {
        if self.workspace == WorkspaceKind::Engine && !self.can_author_engine() {
            self.workspace = WorkspaceKind::Project;
        }
        let available = self.available_editors();
        let (layout, navigation) = match self.workspace {
            WorkspaceKind::Engine => (&mut self.engine_layout, &mut self.engine_navigation),
            WorkspaceKind::Project => (&mut self.project_layout, &mut self.project_navigation),
        };
        match navigation.open(layout, editor, &available) {
            Ok(opened) => {
                self.personal_dirty |= opened.layout_changed;
                let label = available
                    .iter()
                    .find(|item| item.editor == editor)
                    .map(|item| item.label.as_str())
                    .unwrap_or_else(|| editor.label());
                self.status = format!("Active window: {label} (area {})", opened.id);
            }
            Err(error) => self.status = error,
        }
    }

    pub(super) fn active_window(&self) -> Option<u64> {
        match self.workspace {
            WorkspaceKind::Engine => self.engine_navigation.active(),
            WorkspaceKind::Project => self.project_navigation.active(),
        }
    }

    pub(super) fn draw_window_dock(
        &mut self,
        ui: &mut egui::Ui,
        available: &[EditorDescriptor],
    ) -> DockResponse {
        let (layout, navigation) = match self.workspace {
            WorkspaceKind::Engine => (&mut self.engine_layout, &mut self.engine_navigation),
            WorkspaceKind::Project => (&mut self.project_layout, &mut self.project_navigation),
        };
        let restricted = layout.restrict(available);
        navigation.reconcile(layout);
        let mut response = layout.draw(ui, available, navigation.active());
        response.changed |= restricted;
        // Close/reset/restrict may invalidate the focused ID. Reuse the most
        // recently used surviving area, then the first leaf as a stable fallback.
        navigation.reconcile(layout);
        if let Some(id) = response.activated_pane {
            navigation.activate(id, layout);
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::super::dock::{Area, Axis};
    use super::*;

    #[test]
    fn existing_windows_reuse_the_last_active_matching_area() {
        let mut layout = DockLayout::single(EditorType::RustCustom);
        let second = layout.split(1, Axis::Horizontal).unwrap();
        let third = layout.split(second, Axis::Vertical).unwrap();
        layout.choose(third, EditorType::Viewport);
        let before = layout.clone();
        let mut navigation = WindowNavigation::default();
        navigation.activate(second, &layout);
        navigation.activate(third, &layout);
        let opened = navigation
            .open(
                &mut layout,
                EditorType::RustCustom,
                &EditorDescriptor::builtins(true),
            )
            .unwrap();
        assert_eq!(opened.id, second);
        assert!(!opened.layout_changed);
        assert_eq!(layout, before);
        assert_eq!(navigation.active(), Some(second));
        // With no activation history the first matching leaf is deterministic.
        navigation = WindowNavigation::default();
        assert_eq!(
            navigation
                .open(
                    &mut layout,
                    EditorType::RustCustom,
                    &EditorDescriptor::builtins(true)
                )
                .unwrap()
                .id,
            1
        );
    }

    #[test]
    fn absent_window_splits_active_area_and_preserves_existing_contents() {
        let mut layout = DockLayout::default_for(WorkspaceKind::Project);
        let before = layout.panes();
        let active = before.last().unwrap().0;
        let mut navigation = WindowNavigation::default();
        navigation.activate(active, &layout);
        let opened = navigation
            .open(
                &mut layout,
                EditorType::History,
                &EditorDescriptor::builtins(true),
            )
            .unwrap();
        assert!(opened.layout_changed);
        assert_eq!(navigation.active(), Some(opened.id));
        for existing in &before {
            assert!(layout.panes().contains(existing));
        }
        fn is_sibling(area: &Area, active: u64, added: u64) -> bool {
            match area {
                Area::Split {
                    axis: Axis::Horizontal,
                    first,
                    second,
                    ..
                } if matches!(first.as_ref(), Area::Pane { id, .. } if *id == active)
                    && matches!(second.as_ref(), Area::Pane { id, editor: EditorType::History } if *id == added) =>
                {
                    true
                }
                Area::Split { first, second, .. } => {
                    is_sibling(first, active, added) || is_sibling(second, active, added)
                }
                Area::Pane { .. } => false,
            }
        }
        assert!(is_sibling(&layout.root, active, opened.id));
        let same = navigation
            .open(
                &mut layout,
                EditorType::History,
                &EditorDescriptor::builtins(true),
            )
            .unwrap();
        assert_eq!(same.id, opened.id);
        assert!(!same.layout_changed);
        assert_eq!(layout.panes().len(), before.len() + 1);
        layout.validate().unwrap();
    }

    #[test]
    fn stale_focus_and_unavailable_windows_do_not_replace_current_content() {
        let mut layout = DockLayout::default_for(WorkspaceKind::Project);
        let panes = layout.panes();
        let mut navigation = WindowNavigation::default();
        navigation.activate(panes[1].0, &layout);
        navigation.activate(panes[2].0, &layout);
        assert!(layout.close(panes[2].0));
        navigation.reconcile(&layout);
        assert_eq!(navigation.active(), Some(panes[1].0));
        let before = layout.clone();
        assert!(
            navigation
                .open(
                    &mut layout,
                    EditorType::ModuleGraph,
                    &EditorDescriptor::builtins(false)
                )
                .is_err()
        );
        assert_eq!(layout, before);
        assert!(
            navigation
                .open(
                    &mut layout,
                    EditorType::Extension(900),
                    &EditorDescriptor::builtins(true)
                )
                .is_err()
        );
        assert_eq!(layout, before);
        // A restored layout with unrelated IDs discards its stale navigation.
        layout = DockLayout::single(EditorType::Game);
        navigation.reconcile(&layout);
        assert_eq!(navigation.active(), Some(1));
        assert!(navigation.recent.is_empty());
    }

    #[test]
    fn depth_limit_failure_is_transactional_and_existing_window_still_opens() {
        let mut layout = DockLayout::single(EditorType::Viewport);
        let mut active = 1;
        for _ in 0..12 {
            active = layout.split(active, Axis::Horizontal).unwrap();
        }
        layout.validate().unwrap();
        let before = layout.clone();
        let mut navigation = WindowNavigation::default();
        navigation.activate(active, &layout);
        assert!(
            navigation
                .open(
                    &mut layout,
                    EditorType::History,
                    &EditorDescriptor::builtins(true)
                )
                .is_err()
        );
        assert_eq!(layout, before);
        assert_eq!(navigation.active(), Some(active));
        let opened = navigation
            .open(
                &mut layout,
                EditorType::Viewport,
                &EditorDescriptor::builtins(true),
            )
            .unwrap();
        assert_eq!(opened.id, active);
        assert_eq!(layout, before);
    }

    #[test]
    fn capacity_and_module_panel_availability_are_enforced() {
        let mut layout = DockLayout::single(EditorType::Viewport);
        let mut pending = std::collections::VecDeque::from([1]);
        while layout.panes().len() < 24 {
            let pane = pending.pop_front().unwrap();
            let added = layout.split(pane, Axis::Horizontal).unwrap();
            pending.extend([pane, added]);
        }
        let before = layout.clone();
        let mut navigation = WindowNavigation::default();
        assert!(
            navigation
                .open(
                    &mut layout,
                    EditorType::History,
                    &EditorDescriptor::builtins(true)
                )
                .is_err()
        );
        assert_eq!(layout, before);
        assert!(
            navigation
                .open(
                    &mut layout,
                    EditorType::Viewport,
                    &EditorDescriptor::builtins(true)
                )
                .is_ok()
        );
        assert_eq!(layout, before);

        layout = DockLayout::single(EditorType::Viewport);
        navigation = WindowNavigation::default();
        let mut available = EditorDescriptor::builtins(false);
        available.push(EditorDescriptor {
            editor: EditorType::Extension(900),
            label: "My module panel".into(),
            requires_engine: false,
        });
        let panel = navigation
            .open(&mut layout, EditorType::Extension(900), &available)
            .unwrap();
        assert_eq!(navigation.active(), Some(panel.id));
        // Disabling its module converts unavailable areas to the author Scene;
        // the same stable area ID remains a valid destination for the menu.
        let opened = navigation
            .open(
                &mut layout,
                EditorType::Scene,
                &EditorDescriptor::builtins(false),
            )
            .unwrap();
        assert_eq!(opened.id, panel.id);
        assert!(opened.layout_changed);
        assert!(
            layout
                .panes()
                .iter()
                .all(|(_, kind)| *kind == EditorType::Scene)
        );
    }
}
