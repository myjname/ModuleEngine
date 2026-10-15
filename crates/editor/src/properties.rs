//! Schema-driven project/object settings and Engine-owned exposure controls.
use super::*;
use module_engine_core::{Comparison, PropertyScope, PropertyType, RuleCondition, VariantRule};
use serde_json::{Value, json};

fn unused_control_id<T>(values: &BTreeMap<String, T>, base: &str, pass: &str) -> String {
    if !values.contains_key(base) {
        return base.into();
    }
    let preferred = format!("{base}.{pass}");
    if !values.contains_key(&preferred) {
        return preferred;
    }
    for suffix in 2.. {
        let id = format!("{preferred}.{suffix}");
        if !values.contains_key(&id) {
            return id;
        }
    }
    unreachable!()
}

/// Offer only built-in choices that target a real, compatible pass of the
/// active renderer. Existing authored commands win when option IDs coincide.
fn gpu_variant_candidates(
    document: &GraphDocument,
) -> BTreeMap<String, module_engine_core::VariantGroup> {
    let Some(graph) = active_vulkan_node(document).and_then(|node| node.subgraph.as_deref()) else {
        return BTreeMap::new();
    };
    let mut candidates = BTreeMap::new();
    let mut reserved = document.engine.exposure.variants.clone();
    for node in &graph.nodes {
        let base = match node.module.as_str() {
            "gpu.fxaa" => "antialiasing",
            "gpu.tonemap" => "tonemap",
            _ => continue,
        };
        let compatible_command = |command: &module_engine_core::RenderCommand| match command {
            module_engine_core::RenderCommand::SetEnabled { pass, .. } => {
                base == "antialiasing" && pass == &node.id
            }
            module_engine_core::RenderCommand::SetParameter { pass, name, .. } => {
                base == "tonemap" && pass == &node.id && name == "mode"
            }
            _ => false,
        };
        let existing = document.engine.exposure.variants.iter().find(|(_, group)| {
            !group.options.is_empty()
                && group.options.values().all(|option| {
                    !option.commands.is_empty() && option.commands.iter().all(compatible_command)
                })
        });
        let id = existing
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| unused_control_id(&reserved, base, &node.id));
        let mut group = module_engine_core::default_renderer_exposure(&node.id, &node.id)
            .variants
            .remove(base)
            .unwrap();
        if node.module == "gpu.fxaa"
            && node.params.get("enabled").and_then(Value::as_bool) == Some(false)
        {
            group.default = "off".into();
        }
        if node.module == "gpu.tonemap"
            && node.params.get("mode").is_some_and(|value| {
                value.as_str() == Some("reinhard") || value.as_f64() == Some(0.)
            })
        {
            group.default = "reinhard".into();
        }
        if id != base {
            group.label = format!("{} ({})", group.label, node.id);
        }
        if let Some((_, authored)) = existing {
            group.options.extend(authored.options.clone());
            group.default = authored.default.clone();
            group.label = authored.label.clone();
        }
        reserved.insert(id.clone(), group.clone());
        candidates.insert(id, group);
    }
    candidates
}

pub(super) fn expose_new_gpu_pass(document: &mut GraphDocument, pass_id: &str) {
    let Some(node) = active_vulkan_node(document)
        .and_then(|node| node.subgraph.as_deref())
        .and_then(|graph| graph.nodes.iter().find(|node| node.id == pass_id))
        .cloned()
    else {
        return;
    };
    if node.module == "gpu.tonemap"
        && !document
            .engine
            .exposure
            .parameters
            .values()
            .any(|parameter| {
                parameter.target.pass == pass_id && parameter.target.name == "exposure"
            })
    {
        let id = unused_control_id(&document.engine.exposure.parameters, "exposure", pass_id);
        let mut parameter = module_engine_core::default_renderer_exposure(pass_id, pass_id)
            .parameters
            .remove("exposure")
            .unwrap();
        parameter.default = node.params.get("exposure").cloned().unwrap_or(json!(1.));
        if id != "exposure" {
            parameter.label = format!("Exposure ({pass_id})");
        }
        document.engine.exposure.parameters.insert(id, parameter);
    }
    for (id, group) in gpu_variant_candidates(document) {
        let owns_pass = group.options.values().any(|variant| {
            variant.commands.iter().any(|command| {
                matches!(command, module_engine_core::RenderCommand::SetEnabled { pass, .. }
                | module_engine_core::RenderCommand::SetParameter { pass, .. } if pass == pass_id)
            })
        });
        if owns_pass && !document.engine.exposure.variants.contains_key(&id) {
            document
                .project
                .variants
                .entry(id.clone())
                .or_insert_with(|| group.default.clone());
            document.engine.exposure.variants.insert(id, group);
        }
    }
    if matches!(node.module.as_str(), "gpu.fxaa" | "gpu.tonemap") {
        document.engine.initialized = true;
    }
}

pub(crate) fn value_editor(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    kind: &PropertyType,
    value: &mut Value,
) -> bool {
    ui.push_id(id, |ui| {
        ui.horizontal(|ui| {
            ui.label(label);
            match kind {
                PropertyType::Number { min, max } => {
                    let mut number = value.as_f64().unwrap_or(*min);
                    let changed = ui
                        .add(
                            egui::DragValue::new(&mut number)
                                .range(*min..=*max)
                                .speed(((max - min) / 300.0).clamp(0.01, 1.0)),
                        )
                        .changed();
                    if changed {
                        *value = json!(number);
                    }
                    changed
                }
                PropertyType::Boolean => {
                    let mut state = value.as_bool().unwrap_or(false);
                    let changed = ui.checkbox(&mut state, "").changed();
                    if changed {
                        *value = json!(state);
                    }
                    changed
                }
                PropertyType::Enum { choices } => {
                    let mut choice = value.as_str().unwrap_or_default().to_owned();
                    let old = choice.clone();
                    egui::ComboBox::from_id_salt("value")
                        .selected_text(&choice)
                        .show_ui(ui, |ui| {
                            for option in choices {
                                ui.selectable_value(&mut choice, option.clone(), option);
                            }
                        });
                    if choice != old {
                        *value = json!(choice);
                        true
                    } else {
                        false
                    }
                }
                PropertyType::Text { max_len } => {
                    let mut text = value.as_str().unwrap_or_default().to_owned();
                    let changed = ui
                        .add(egui::TextEdit::singleline(&mut text).char_limit(*max_len))
                        .changed();
                    if changed {
                        *value = json!(text);
                    }
                    changed
                }
                PropertyType::ObjectReference { optional, .. } => {
                    let mut object = value.as_u64().unwrap_or_default();
                    let changed = ui
                        .add(egui::DragValue::new(&mut object).range(1..=u64::MAX))
                        .changed();
                    if changed {
                        *value = json!(object);
                    }
                    if *optional && ui.small_button("Clear").clicked() {
                        *value = Value::Null;
                        true
                    } else {
                        changed
                    }
                }
                PropertyType::AssetReference { optional, .. } => {
                    let mut reference = value.as_str().unwrap_or_default().to_owned();
                    let changed = ui.text_edit_singleline(&mut reference).changed();
                    if changed {
                        *value = if *optional && reference.is_empty() {
                            Value::Null
                        } else {
                            json!(reference)
                        };
                    }
                    if *optional && ui.small_button("Clear").clicked() {
                        *value = Value::Null;
                        true
                    } else {
                        changed
                    }
                }
            }
        })
        .inner
    })
    .inner
}
impl Editor {
    pub(super) fn commit_properties(&mut self, mut candidate: GraphDocument, engine: bool) -> bool {
        if self.runtime.is_some()
            && candidate
                .properties
                .active
                .objects
                .keys()
                .chain(candidate.properties.active.components.keys())
                .any(|id| !self.authored_object_ids.contains(id))
        {
            self.status="Persistent component fields require startup/authored object IDs; runtime-spawned objects need ScenePersistence".into();
            return false;
        }
        let result = module_engine_core::reconcile_properties(&mut candidate, &self.registry)
            .and_then(|_| module_engine_core::validate_settings(&candidate, &self.registry))
            .and_then(|_| self.validate_candidate(&candidate, engine));
        if let Err(error) = result {
            self.status = error;
            return false;
        }
        let engine_changed = candidate.engine != self.document.engine;
        // Project values may already target newly added passes while the graph
        // is still a draft. Keep editing the document; the last good Runtime
        // cannot accept those new IDs until its replacement is ready.
        let renderer_pending = self
            .document
            .root
            .nodes
            .iter()
            .any(|node| node.module == "render.vulkan")
            && (self.preview_builder.is_pending() || self.preview_builder.error.is_some());
        if !engine_changed
            && !renderer_pending
            && let Some(runtime) = &mut self.runtime
        {
            let previous_project = self.document.project.clone();
            let previous_properties = self.document.properties.clone();
            let result = runtime
                .set_project_configuration(candidate.project.clone())
                .and_then(|_| runtime.set_properties(candidate.properties.clone()))
                .and_then(|_| runtime.apply_project_settings(self.time));
            if let Err(error) = result {
                let _ = runtime.set_project_configuration(previous_project);
                let _ = runtime.set_properties(previous_properties);
                let _ = runtime.apply_project_settings(self.time);
                self.status = error;
                return false;
            }
        }
        self.document = candidate;
        for pane in self
            .engine_panes
            .values_mut()
            .chain(self.project_panes.values_mut())
        {
            pane.last_render = -1.;
        }
        self.last_render = -1.;
        if engine {
            self.policy_baseline = self.document.clone();
        }
        self.changed(engine_changed || renderer_pending);
        true
    }
    pub(super) fn project_settings_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("General Project Settings");
        let mut candidate = self.document.clone();
        let mut changed = false;
        ui.label("Engine-defined ranges and ready variants");
        for (id, exposed) in &self.document.engine.exposure.parameters {
            let mut value = candidate
                .project
                .parameters
                .get(id)
                .unwrap_or(&exposed.default)
                .clone();
            if value_editor(ui, id, &exposed.label, &exposed.value_type, &mut value) {
                match module_engine_core::set_project_parameter(&mut candidate, id, value) {
                    Ok(()) => changed = true,
                    Err(error) => self.status = error,
                }
            }
        }
        for (id, group) in &self.document.engine.exposure.variants {
            ui.push_id(id, |ui| {
                let mut selected = candidate
                    .project
                    .variants
                    .get(id)
                    .unwrap_or(&group.default)
                    .clone();
                let previous = selected.clone();
                egui::ComboBox::from_id_salt("ready-variant")
                    .selected_text(format!("{}: {}", group.label, selected))
                    .show_ui(ui, |ui| {
                        for (key, variant) in &group.options {
                            ui.selectable_value(&mut selected, key.clone(), &variant.label);
                        }
                    });
                if selected != previous {
                    match module_engine_core::set_project_variant(&mut candidate, id, &selected) {
                        Ok(()) => changed = true,
                        Err(error) => self.status = error,
                    }
                }
            });
        }
        let schemas =
            module_engine_core::visible_property_schemas(&self.document, &self.registry, None);
        for (owner, schema) in schemas {
            let store = match schema.scope {
                PropertyScope::Global => &mut candidate.properties.active.global,
                PropertyScope::Module => candidate
                    .properties
                    .active
                    .modules
                    .entry(owner)
                    .or_default(),
                _ => continue,
            };
            let mut value = store.get(&schema.id).unwrap_or(&schema.default).clone();
            if value_editor(
                ui,
                &schema.id,
                &schema.label,
                &schema.value_type,
                &mut value,
            ) {
                store.insert(schema.id, value);
                changed = true;
            }
        }
        ui.separator();
        ui.label("Conditions select existing variants; first matching rule per group wins.");
        let groups = self.document.engine.exposure.variants.clone();
        let parameters = self.document.engine.exposure.parameters.clone();
        let mut remove = None;
        for (index, rule) in candidate.project.rules.iter_mut().enumerate() {
            ui.push_id(("rule", index), |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("Rule {}", index + 1));
                    if ui.small_button("Remove").clicked() {
                        remove = Some(index);
                    }
                });
                let old = rule.clone();
                egui::ComboBox::from_id_salt("group")
                    .selected_text(&rule.group)
                    .show_ui(ui, |ui| {
                        for group in groups.keys() {
                            ui.selectable_value(&mut rule.group, group.clone(), group);
                        }
                    });
                if rule.group != old.group
                    && let Some(group) = groups.get(&rule.group)
                {
                    rule.variant = group.default.clone();
                }
                egui::ComboBox::from_id_salt("variant")
                    .selected_text(&rule.variant)
                    .show_ui(ui, |ui| {
                        if let Some(group) = groups.get(&rule.group) {
                            for (id, variant) in &group.options {
                                ui.selectable_value(&mut rule.variant, id.clone(), &variant.label);
                            }
                        }
                    });
                let mut time = matches!(rule.condition, RuleCondition::Time { .. });
                let previous = time;
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut time, true, "Time");
                    ui.selectable_value(&mut time, false, "Parameter");
                });
                if time != previous {
                    if time {
                        rule.condition = RuleCondition::Time {
                            op: Comparison::GreaterOrEqual,
                            value: 1.0,
                        };
                    } else if let Some((id, p)) = parameters.iter().next() {
                        rule.condition = RuleCondition::Parameter {
                            parameter: id.clone(),
                            op: Comparison::GreaterOrEqual,
                            value: p.default.clone(),
                        };
                    }
                }
                let op = match &mut rule.condition {
                    RuleCondition::Time { op, value } => {
                        ui.add(
                            egui::DragValue::new(value)
                                .range(0.0..=1.0e6)
                                .prefix("seconds = "),
                        );
                        op
                    }
                    RuleCondition::Parameter {
                        parameter,
                        op,
                        value,
                    } => {
                        egui::ComboBox::from_id_salt("parameter")
                            .selected_text(parameter.as_str())
                            .show_ui(ui, |ui| {
                                for id in parameters.keys() {
                                    ui.selectable_value(parameter, id.clone(), id);
                                }
                            });
                        if let Some(schema) = parameters.get(parameter) {
                            value_editor(
                                ui,
                                "comparison-value",
                                "Value",
                                &schema.value_type,
                                value,
                            );
                        }
                        op
                    }
                };
                egui::ComboBox::from_id_salt("comparison")
                    .selected_text(format!("{op:?}"))
                    .show_ui(ui, |ui| {
                        for choice in [
                            Comparison::Equals,
                            Comparison::NotEquals,
                            Comparison::Less,
                            Comparison::LessOrEqual,
                            Comparison::Greater,
                            Comparison::GreaterOrEqual,
                        ] {
                            ui.selectable_value(op, choice, format!("{choice:?}"));
                        }
                    });
                changed |= *rule != old;
                ui.separator();
            });
        }
        if let Some(index) = remove {
            candidate.project.rules.remove(index);
            changed = true;
        }
        if let Some((group, definition)) = groups.iter().next()
            && ui.button("Add condition").clicked()
        {
            candidate.project.rules.push(VariantRule {
                group: group.clone(),
                variant: definition.default.clone(),
                condition: RuleCondition::Time {
                    op: Comparison::GreaterOrEqual,
                    value: 1.0,
                },
            });
            changed = true;
        }
        if self.document.engine.exposure.parameters.is_empty() && groups.is_empty() {
            ui.label("Engine author has not exposed project controls for this engine.");
        }
        if changed {
            self.commit_properties(candidate, false);
        }
    }
    pub(super) fn object_tree_panel(&mut self, ui: &mut egui::Ui, selected: &mut Option<u64>) {
        ui.heading("Object Tree");
        let Some(runtime) = &self.runtime else {
            ui.label("No active world");
            return;
        };
        let objects = match runtime.objects() {
            Ok(objects) => objects,
            Err(error) => {
                ui.label(error);
                return;
            }
        };
        fn tree(
            ui: &mut egui::Ui,
            objects: &[module_engine_core::WorldObject],
            parent: Option<u64>,
            selected: &mut Option<u64>,
            depth: usize,
            clicked: &mut Option<u64>,
        ) {
            if depth > 64 {
                return;
            }
            for object in objects.iter().filter(|o| o.parent == parent) {
                ui.push_id(object.id, |ui| {
                    ui.horizontal(|ui| {
                        ui.add_space(depth as f32 * 12.0);
                        if ui
                            .selectable_label(
                                *selected == Some(object.id),
                                format!("Object {}", object.id),
                            )
                            .clicked()
                        {
                            *selected = Some(object.id);
                            *clicked = Some(object.id);
                        }
                    });
                });
                tree(ui, objects, Some(object.id), selected, depth + 1, clicked);
            }
        }
        let mut clicked = None;
        tree(ui, &objects, None, selected, 0, &mut clicked);
        if let Some(id) = clicked {
            self.active_object = Some(id);
        }
    }
    pub(super) fn object_inspector_panel(
        &mut self,
        ui: &mut egui::Ui,
        selected: &mut Option<u64>,
        pinned: &mut bool,
    ) {
        ui.heading("Object / Components");
        ui.checkbox(pinned, "Pin this object in this area");
        if !*pinned {
            *selected = self.active_object;
        }
        ui.horizontal(|ui| {
            if ui.button("Use tree selection").clicked() {
                *selected = self.active_object;
            }
            ui.label(
                selected
                    .map(|id| format!("Object {id}"))
                    .unwrap_or_else(|| "No selection".into()),
            );
        });
        if selected.is_none() {
            *selected = self.active_object;
        }
        let Some(id) = *selected else {
            ui.label("Select an object in an Object Tree area");
            return;
        };
        let object = self
            .runtime
            .as_ref()
            .and_then(|r| r.objects().ok())
            .and_then(|objects| objects.into_iter().find(|o| o.id == id));
        let Some(mut object) = object else {
            ui.label("Selected object no longer participates");
            return;
        };
        ui.label("Preview only: Transform / Color are not saved.");
        let mut world_changed = false;
        ui.collapsing("Transform", |ui| {
            for (label, values) in [
                ("Position", &mut object.transform.translation),
                ("Rotation", &mut object.transform.rotation),
                ("Scale", &mut object.transform.scale),
            ] {
                ui.horizontal(|ui| {
                    ui.label(label);
                    for value in values.iter_mut() {
                        world_changed |= ui.add(egui::DragValue::new(value).speed(0.02)).changed();
                    }
                });
            }
        });
        ui.horizontal(|ui| {
            ui.label("Color");
            for value in &mut object.color {
                world_changed |= ui.add(egui::DragValue::new(value).range(0..=255)).changed();
            }
        });
        if world_changed && let Some(runtime) = &mut self.runtime {
            if let Err(error) = runtime.apply_controls(&[
                ControlCommand::World(module_engine_core::WorldCommand::SetTransform {
                    id,
                    transform: object.transform,
                }),
                ControlCommand::World(module_engine_core::WorldCommand::SetColor {
                    id,
                    color: object.color,
                }),
            ]) {
                self.status = error;
            }
            self.last_render = -1.;
        }
        if !self.authored_object_ids.contains(&id) {
            ui.label("Runtime-spawned object: persistent component fields require authored scene storage (future ScenePersistence).");
            return;
        }
        let mut candidate = self.document.clone();
        let mut changed = false;
        for (owner, schema) in
            module_engine_core::visible_property_schemas(&self.document, &self.registry, Some(id))
        {
            let store = match &schema.scope {
                PropertyScope::Object => candidate.properties.active.objects.entry(id).or_default(),
                PropertyScope::Component { component } => candidate
                    .properties
                    .active
                    .components
                    .entry(id)
                    .or_default()
                    .entry(component.clone())
                    .or_default(),
                PropertyScope::Module => candidate
                    .properties
                    .active
                    .modules
                    .entry(owner)
                    .or_default(),
                _ => continue,
            };
            let mut value = store.get(&schema.id).unwrap_or(&schema.default).clone();
            if value_editor(
                ui,
                &schema.id,
                &schema.label,
                &schema.value_type,
                &mut value,
            ) {
                store.insert(schema.id, value);
                changed = true;
            }
        }
        ui.separator();
        ui.label("Active module components");
        let active = module_engine_core::effective_modules(&self.document);
        let mut components = BTreeMap::new();
        for descriptor in self.registry.descriptors() {
            if !active.contains(&descriptor.id) {
                continue;
            }
            for schema in &descriptor.properties {
                if let PropertyScope::Component { component } = &schema.scope {
                    components
                        .entry(component.clone())
                        .or_insert(descriptor.label.clone());
                }
            }
        }
        for (component, label) in components {
            ui.push_id(&component, |ui| {
                let attached = candidate
                    .properties
                    .active
                    .components
                    .get(&id)
                    .is_some_and(|fields| fields.contains_key(&component));
                if ui
                    .button(format!(
                        "{} {label} / {component}",
                        if attached { "Detach" } else { "Attach" }
                    ))
                    .clicked()
                {
                    let result = if attached {
                        module_engine_core::detach_component(
                            &mut candidate,
                            &self.registry,
                            id,
                            &component,
                        )
                    } else {
                        module_engine_core::attach_component(
                            &mut candidate,
                            &self.registry,
                            id,
                            &component,
                        )
                    };
                    match result {
                        Ok(()) => changed = true,
                        Err(error) => self.status = error,
                    }
                }
            });
        }
        if changed {
            self.commit_properties(candidate, false);
        }
    }
    pub(super) fn engine_settings_panel(&mut self, ui: &mut egui::Ui) {
        if !self.can_author_engine() {
            ui.label("Engine authoring requires Full profile and complete matching sources");
            return;
        }
        ui.heading("Engine Exposure");
        let mut candidate = self.document.clone();
        let mut changed = false;
        ui.label("Only these definitions are editable from Project settings.");
        let mut remove_parameter = None;
        for (id, definition) in &self.document.engine.exposure.parameters {
            ui.horizontal(|ui| {
                ui.label(format!("{} ({id})", definition.label));
                if ui.small_button("Stop exposing").clicked() {
                    remove_parameter = Some(id.clone());
                }
            });
        }
        if let Some(id) = remove_parameter {
            candidate.engine.exposure.parameters.remove(&id);
            changed = true;
        }
        let exposed_targets = candidate
            .engine
            .exposure
            .parameters
            .values()
            .map(|p| (p.target.pass.clone(), p.target.name.clone()))
            .collect::<std::collections::BTreeSet<_>>();
        let pass_nodes = active_vulkan_node(&self.document)
            .and_then(|node| node.subgraph.as_deref())
            .map(|graph| graph.nodes.clone())
            .unwrap_or_default();
        egui::CollapsingHeader::new("Expose an existing GPU parameter").show(ui, |ui| {
            for node in pass_nodes {
                let manifest = if node.module == "gpu.custom" {
                    node.params.get("manifest").and_then(|m| {
                        serde_json::from_value::<module_engine_vulkan::PassManifest>(m.clone()).ok()
                    })
                } else {
                    module_engine_vulkan::builtin_manifest(&node.module)
                };
                if let Some(manifest) = manifest {
                    for parameter in manifest.parameters {
                        if (parameter == "mode" && node.module != "gpu.custom")
                            || exposed_targets.contains(&(node.id.clone(), parameter.clone()))
                        {
                            continue;
                        }
                        if ui
                            .button(format!("Expose {}.{parameter}", node.id))
                            .clicked()
                        {
                            let id = unused_control_id(
                                &candidate.engine.exposure.parameters,
                                &format!("{}.{}", node.id, parameter),
                                &node.id,
                            );
                            let default = node
                                .params
                                .get(&parameter)
                                .and_then(Value::as_f64)
                                .unwrap_or(1.0);
                            candidate.engine.exposure.parameters.insert(
                                id.clone(),
                                module_engine_core::ExposedParameter {
                                    label: id,
                                    value_type: PropertyType::Number {
                                        min: default.min(0.0),
                                        max: default.max(8.0),
                                    },
                                    default: json!(default),
                                    target: module_engine_core::RenderParameterTarget {
                                        pass: node.id.clone(),
                                        name: parameter,
                                    },
                                },
                            );
                            changed = true;
                        }
                    }
                }
            }
        });
        for (id, parameter) in &mut candidate.engine.exposure.parameters {
            ui.push_id(id, |ui| {
                ui.label(format!(
                    "{} → {}.{}",
                    parameter.label, parameter.target.pass, parameter.target.name
                ));
                changed |= ui.text_edit_singleline(&mut parameter.label).changed();
                if let PropertyType::Number { min, max } = &mut parameter.value_type {
                    ui.horizontal(|ui| {
                        ui.label("Allowed min / max");
                        changed |= ui.add(egui::DragValue::new(min).speed(0.02)).changed();
                        changed |= ui.add(egui::DragValue::new(max).speed(0.02)).changed();
                    });
                }
                changed |= value_editor(
                    ui,
                    "default",
                    "Default",
                    &parameter.value_type,
                    &mut parameter.default,
                );
                ui.separator();
            });
        }
        let ready = gpu_variant_candidates(&candidate);
        egui::CollapsingHeader::new("Expose existing GPU variants").show(ui, |ui| {
            for (id, group) in &ready {
                if !candidate.engine.exposure.variants.contains_key(id)
                    && ui.button(format!("Expose {}", group.label)).clicked()
                {
                    candidate
                        .engine
                        .exposure
                        .variants
                        .insert(id.clone(), group.clone());
                    changed = true;
                }
            }
        });
        for (id, group) in &mut candidate.engine.exposure.variants {
            ui.push_id(id, |ui| {
                ui.label(format!("Allowed {} variants", group.label));
                let candidates = ready
                    .get(id)
                    .map(|g| g.options.clone())
                    .unwrap_or_else(|| group.options.clone());
                for (option, variant) in candidates {
                    let mut allowed = group.options.contains_key(&option);
                    if ui.checkbox(&mut allowed, &variant.label).changed() {
                        if allowed {
                            group.options.insert(option, variant);
                        } else {
                            group.options.remove(&option);
                        }
                        if !group.options.contains_key(&group.default)
                            && let Some(id) = group.options.keys().next()
                        {
                            group.default = id.clone();
                        }
                        changed = true;
                    }
                }
                egui::ComboBox::from_id_salt("default-variant")
                    .selected_text(format!("Default: {}", group.default))
                    .show_ui(ui, |ui| {
                        for (id, option) in &group.options {
                            changed |= ui
                                .selectable_value(&mut group.default, id.clone(), &option.label)
                                .changed();
                        }
                    });
                ui.separator();
            });
        }
        // Hiding a definition is an Engine edit. Its obsolete Project value or
        // condition must disappear in the same history entry.
        candidate
            .engine
            .exposure
            .variants
            .retain(|_, group| !group.options.is_empty());
        ui.heading("Participating feature modules");
        for descriptor in self.registry.descriptors() {
            if descriptor.properties.is_empty() {
                continue;
            }
            let mut included = candidate.engine.enabled_modules.contains(&descriptor.id);
            if ui.checkbox(&mut included, &descriptor.label).changed() {
                if included {
                    candidate
                        .engine
                        .enabled_modules
                        .insert(descriptor.id.clone());
                } else {
                    candidate.engine.enabled_modules.remove(&descriptor.id);
                }
                changed = true;
            }
        }
        if changed {
            candidate
                .project
                .parameters
                .retain(|id, _| candidate.engine.exposure.parameters.contains_key(id));
            candidate.project.variants.retain(|id, option| {
                candidate
                    .engine
                    .exposure
                    .variants
                    .get(id)
                    .is_some_and(|group| group.options.contains_key(option))
            });
            candidate.project.rules.retain(|rule| {
                candidate
                    .engine
                    .exposure
                    .variants
                    .get(&rule.group)
                    .is_some_and(|group| group.options.contains_key(&rule.variant))
                    && !matches!(&rule.condition, RuleCondition::Parameter { parameter, .. }
                        if !candidate.engine.exposure.parameters.contains_key(parameter))
            });
            candidate.engine.initialized = true;
            self.commit_properties(candidate, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_use_real_passes_and_preserve_authored_variants_and_project_choices() {
        let mut document = crate::host::gpu_document(WorldKind::Objects, RenderPath::Forward);
        crate::host::ensure_authoring_configuration(&mut document);
        document
            .engine
            .exposure
            .variants
            .get_mut("antialiasing")
            .unwrap()
            .options
            .get_mut("off")
            .unwrap()
            .label = "Custom authored off".into();
        document
            .project
            .variants
            .insert("antialiasing".into(), "off".into());
        document.project.rules.push(VariantRule {
            group: "antialiasing".into(),
            variant: "fxaa".into(),
            condition: RuleCondition::Time {
                op: Comparison::Greater,
                value: 2.,
            },
        });
        let authored = document.clone();
        let graph = document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.module == "render.vulkan")
            .unwrap()
            .subgraph
            .as_deref_mut()
            .unwrap();
        graph.nodes.push(make_node(
            "new-aa",
            "gpu.fxaa",
            "Second FXAA",
            [0., 0.],
            json!({"enabled":false}),
        ));
        expose_new_gpu_pass(&mut document, "new-aa");
        assert_eq!(
            document.engine.exposure.variants["antialiasing"],
            authored.engine.exposure.variants["antialiasing"]
        );
        assert_eq!(document.project.variants["antialiasing"], "off");
        assert_eq!(document.project.rules, authored.project.rules);
        assert_eq!(
            document.engine.exposure.parameters,
            authored.engine.exposure.parameters
        );
        let id = "antialiasing.new-aa";
        assert_eq!(document.project.variants[id], "off");
        assert!(document.engine.exposure.variants[id].options.values().all(|variant| variant.commands.iter().all(|command| {
            matches!(command, module_engine_core::RenderCommand::SetEnabled { pass, .. } if pass == "new-aa")
        })));
        let once = document.clone();
        expose_new_gpu_pass(&mut document, "new-aa");
        assert_eq!(document, once);
        let candidates = gpu_variant_candidates(&document);
        assert_eq!(
            candidates["antialiasing"].options["off"].label,
            "Custom authored off"
        );
        module_engine_core::validate_settings(&document, &crate::host::registry().unwrap())
            .unwrap();
    }

    #[test]
    fn preset_candidates_require_compatible_passes_and_ignore_alternative_renderer() {
        let mut document = crate::host::gpu_document(WorldKind::Objects, RenderPath::Forward);
        let mut alternative = active_vulkan_node(&document).unwrap().clone();
        alternative.id = "alternative".into();
        let graph = document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.module == "render.vulkan")
            .unwrap()
            .subgraph
            .as_deref_mut()
            .unwrap();
        graph
            .nodes
            .retain(|node| !matches!(node.module.as_str(), "gpu.fxaa" | "gpu.tonemap"));
        document.root.nodes.push(alternative);
        assert!(gpu_variant_candidates(&document).is_empty());
        let before = document.clone();
        expose_new_gpu_pass(&mut document, "aa");
        assert_eq!(document, before);
    }

    #[test]
    fn new_tonemap_presets_match_the_shader_mode_abi_and_numeric_default() {
        let mut document = crate::host::gpu_document(WorldKind::Objects, RenderPath::Forward);
        let graph = document
            .root
            .nodes
            .iter_mut()
            .find(|node| node.module == "render.vulkan")
            .unwrap()
            .subgraph
            .as_deref_mut()
            .unwrap();
        graph
            .nodes
            .iter_mut()
            .find(|node| node.module == "gpu.tonemap")
            .unwrap()
            .params["mode"] = json!(0.);
        expose_new_gpu_pass(&mut document, "tonemap");
        let group = &document.engine.exposure.variants["tonemap"];
        assert_eq!(group.default, "reinhard");
        assert_eq!(document.project.variants["tonemap"], "reinhard");
        assert_eq!(
            group.options["aces"].commands,
            vec![module_engine_core::RenderCommand::SetParameter {
                pass: "tonemap".into(),
                name: "mode".into(),
                value: json!(1.),
            }]
        );
        assert_eq!(
            group.options["reinhard"].commands,
            vec![module_engine_core::RenderCommand::SetParameter {
                pass: "tonemap".into(),
                name: "mode".into(),
                value: json!(0.),
            }]
        );
    }
}
