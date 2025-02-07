/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */

use std::collections::HashSet;
use std::fmt;

use common::InterfaceName;
use intern::string_key::Intern;
use intern::string_key::StringKey;
use lazy_static::lazy_static;
use relay_config::SchemaConfig;
use rustc_hash::FxHashSet;
use schema::SDLSchema;
use schema::Schema;

use crate::definitions::ArgumentChange;
use crate::definitions::DefinitionChange;
use crate::definitions::SchemaChange;
use crate::definitions::Type;
use crate::definitions::TypeChange;

// This enum is very similar to the schema Type enum but uses StringKey instead of id
#[derive(Eq, PartialEq, Hash)]
pub enum IncrementalBuildSchemaChange {
    Enum(StringKey),
}

impl fmt::Debug for IncrementalBuildSchemaChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IncrementalBuildSchemaChange::Enum(name) => write!(f, "enum({})", name),
        }
    }
}

#[derive(PartialEq)]
pub enum SchemaChangeSafety {
    Unsafe,
    SafeWithIncrementalBuild(FxHashSet<IncrementalBuildSchemaChange>),
    Safe,
}

impl fmt::Debug for SchemaChangeSafety {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchemaChangeSafety::Unsafe => write!(f, "Unsafe"),
            SchemaChangeSafety::SafeWithIncrementalBuild(changes) => {
                write!(f, "SafeWithIncrementalBuild({:?})", changes)
            }
            SchemaChangeSafety::Safe => write!(f, "Safe"),
        }
    }
}

// Return if the changes are safe to skip full rebuild or need an incremental build.
impl SchemaChange {
    pub fn get_safety(
        self: SchemaChange,
        schema: &SDLSchema,
        schema_config: &SchemaConfig,
    ) -> SchemaChangeSafety {
        let mut needs_incremental_build: FxHashSet<IncrementalBuildSchemaChange> =
            HashSet::default();
        match self {
            SchemaChange::None => SchemaChangeSafety::Unsafe,
            SchemaChange::GenericChange => SchemaChangeSafety::Unsafe,
            SchemaChange::InvalidSchema => SchemaChangeSafety::Unsafe,
            SchemaChange::DefinitionChanges(changes) => {
                for change in changes {
                    match change {
                        DefinitionChange::ObjectChanged {
                            changed,
                            added,
                            removed,
                            interfaces_added,
                            interfaces_removed,
                            ..
                        } => {
                            if !interfaces_added.is_empty()
                                || !interfaces_removed.is_empty()
                                || !is_field_changes_safe(
                                    &added,
                                    &removed,
                                    &changed,
                                    schema_config.node_interface_id_field,
                                )
                            {
                                return SchemaChangeSafety::Unsafe;
                            }
                        }
                        DefinitionChange::InterfaceChanged {
                            changed,
                            added,
                            removed,
                            ..
                        } => {
                            if !is_field_changes_safe(
                                &added,
                                &removed,
                                &changed,
                                schema_config.node_interface_id_field,
                            ) {
                                return SchemaChangeSafety::Unsafe;
                            }
                        }
                        DefinitionChange::ObjectAdded(name) => {
                            if !is_object_add_safe(name, schema, schema_config) {
                                return SchemaChangeSafety::Unsafe;
                            }
                        }
                        // safe changes
                        DefinitionChange::InterfaceAdded(_)
                        | DefinitionChange::EnumAdded(_)
                        | DefinitionChange::ScalarAdded(_)
                        | DefinitionChange::UnionAdded(_)
                        | DefinitionChange::InputObjectAdded(_) => {}

                        // safe with incremental build changes
                        DefinitionChange::EnumChanged { name }
                        | DefinitionChange::EnumRemoved(name) => {
                            needs_incremental_build
                                .insert(IncrementalBuildSchemaChange::Enum(name));
                        }

                        // unsafe changes
                        DefinitionChange::UnionChanged { .. }
                        | DefinitionChange::UnionRemoved(_)
                        | DefinitionChange::ScalarRemoved(_)
                        | DefinitionChange::InputObjectChanged { .. }
                        | DefinitionChange::InputObjectRemoved(_)
                        | DefinitionChange::InterfaceRemoved(_)
                        | DefinitionChange::ObjectRemoved(_) => return SchemaChangeSafety::Unsafe,
                    }
                }
                if needs_incremental_build.is_empty() {
                    SchemaChangeSafety::Safe
                } else {
                    SchemaChangeSafety::SafeWithIncrementalBuild(needs_incremental_build)
                }
            }
        }
    }
}

lazy_static! {
    static ref JS_FIELD_KEY: StringKey = "js".intern();
    static ref NODE_INTERFACE_KEY: InterfaceName = InterfaceName("Node".intern());
}

/// If the type has an `id` field and the type implements interfaces
/// other than `Node`, then the change isn't safe:
/// If the object type implements an `Actor` interfaces (for example).
/// We may need to add an inline spread with
///   ... on ThisNewType {
///     id
///   }
/// But we have a special case for `Node`. The `id` field is automatically
/// added to the selection for all types that implements `Node`.
fn is_object_add_safe(name: StringKey, schema: &SDLSchema, schema_config: &SchemaConfig) -> bool {
    let id_name = schema_config.node_interface_id_field;

    if let Some(schema::Type::Object(id)) = schema.get_type(name) {
        let object = schema.object(id);
        if object
            .fields
            .iter()
            .any(|id| schema.field(*id).name.item == id_name)
            && object
                .interfaces
                .iter()
                .any(|id| schema.interface(*id).name.item != *NODE_INTERFACE_KEY)
        {
            return false;
        }
    } else {
        panic!(
            "The object '{}' was not found in the schema during schema change detection.",
            name
        );
    }
    true
}

fn is_field_changes_safe(
    added: &[TypeChange],
    removed: &[TypeChange],
    changed: &[ArgumentChange],
    id_name: StringKey,
) -> bool {
    if !removed.is_empty() {
        return false;
    }

    // Special fields might change the compile output:
    // - `id` added to an object makes the compiler select that field
    // - `js` field added to a type might change 3D code generation
    if added
        .iter()
        .any(|add| add.name == id_name || add.name == *JS_FIELD_KEY)
    {
        return false;
    }

    // Addition of optional field arg is safe
    for change in changed {
        if !change.removed.is_empty() {
            return false;
        }
        for add in &change.added {
            if let Type::NonNull(_) = add.type_ {
                return false;
            }
        }
    }
    true
}
