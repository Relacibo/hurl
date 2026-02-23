/*
 * Hurl (https://hurl.dev)
 * Copyright (C) 2026 Orange
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *          http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 *
 */

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use hurl_core::ast::{BindingExpr, BindingParam, SourceInfo};

use crate::util::path::ContextDir;

use super::error::{RunnerError, RunnerErrorKind};
use super::template;
use super::value::Value;
use super::variable::VariableSet;

/// Tracks which variables are synced to which files
#[derive(Clone, Debug, Default)]
pub struct BoundVariables {
    /// Maps variable name to file path
    pub mappings: HashMap<String, String>,
}

impl BoundVariables {
    pub fn new() -> Self {
        BoundVariables {
            mappings: HashMap::new(),
        }
    }

    /// Processes binding parameters and loads variables from files
    pub fn process_bindings(
        &mut self,
        binding_params: &[BindingParam],
        variables: &mut VariableSet,
        context_dir: &ContextDir,
    ) -> Result<(), RunnerError> {
        for param in binding_params {
            // Render the variable name
            let var_name = template::eval_template(&param.name, variables)?;

            match &param.value {
                BindingExpr::File { filename, .. } => {
                    // Render the filename (supports template variables like {env})
                    let filename = template::eval_template(filename, variables)?;

                    // Convert to path relative to context_dir
                    let file_path = context_dir.resolved_path(Path::new(&filename));

                    // Always store/update the mapping
                    self.mappings
                        .insert(var_name.clone(), file_path.to_string_lossy().to_string());

                    // Try to load the file content into the variable (only if file exists)
                    if file_path.exists() {
                        match fs::read_to_string(&file_path) {
                            Ok(content) => {
                                let content = content.trim_end_matches('\n').to_string();
                                variables.insert(var_name, Value::String(content));
                            }
                            Err(_e) => {
                                let source_info = param.name.source_info;
                                return Err(RunnerError::new(
                                    source_info,
                                    RunnerErrorKind::FileReadAccess {
                                        path: file_path.clone(),
                                    },
                                    false,
                                ));
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Writes a variable to its synced file if it's registered
    pub fn bind_variable(
        &self,
        var_name: &str,
        value: &Value,
        source_info: SourceInfo,
    ) -> Result<(), RunnerError> {
        if let Some(file_path) = self.mappings.get(var_name) {
            let value_str = match value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };

            // Create parent directories if needed
            let path = Path::new(file_path);
            if let Some(parent) = path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent).map_err(|e| {
                        RunnerError::new(
                            source_info,
                            RunnerErrorKind::FileWriteAccess {
                                path: PathBuf::from(file_path),
                                error: e.to_string(),
                            },
                            false,
                        )
                    })?;
                }
            }

            // Atomic write: write to temp file, then rename
            let temp_path = format!("{}.tmp", file_path);
            let mut file = fs::File::create(&temp_path).map_err(|e| {
                RunnerError::new(
                    source_info,
                    RunnerErrorKind::FileWriteAccess {
                        path: PathBuf::from(file_path),
                        error: e.to_string(),
                    },
                    false,
                )
            })?;

            file.write_all(value_str.as_bytes()).map_err(|e| {
                RunnerError::new(
                    source_info,
                    RunnerErrorKind::FileWriteAccess {
                        path: PathBuf::from(file_path),
                        error: e.to_string(),
                    },
                    false,
                )
            })?;

            // Ensure data is written to disk
            file.sync_all().map_err(|e| {
                RunnerError::new(
                    source_info,
                    RunnerErrorKind::FileWriteAccess {
                        path: PathBuf::from(file_path),
                        error: e.to_string(),
                    },
                    false,
                )
            })?;

            drop(file);

            // Atomic rename
            fs::rename(&temp_path, file_path).map_err(|e| {
                RunnerError::new(
                    source_info,
                    RunnerErrorKind::FileWriteAccess {
                        path: PathBuf::from(file_path),
                        error: e.to_string(),
                    },
                    false,
                )
            })?;

            // Set restrictive permissions (600 - owner read/write only)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(file_path).unwrap().permissions();
                perms.set_mode(0o600);
                let _ = fs::set_permissions(file_path, perms);
            }
        }
        Ok(())
    }

    /// Returns true if a variable is registered for syncing
    pub fn is_bound(&self, var_name: &str) -> bool {
        self.mappings.contains_key(var_name)
    }
}
