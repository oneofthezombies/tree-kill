use crate::core::{
    ChildProcessIdMap, ChildProcessIdMapFilter, Config, KillOutput, Killable, Output, Outputs,
    ProcessId, ProcessIds, ProcessInfo, ProcessInfoMap, ProcessInfos, Result,
};
use tracing::debug;

#[cfg(target_os = "linux")]
use crate::linux as imp;
#[cfg(target_os = "macos")]
use crate::macos as imp;
#[cfg(windows)]
use crate::windows as imp;

pub(crate) fn get_available_max_process_id() -> u32 {
    imp::AVAILABLE_MAX_PROCESS_ID
}

/// Create a map from parent process id to child process ids.
pub(crate) fn get_child_process_id_map(
    process_infos: &[ProcessInfo],
    filter: ChildProcessIdMapFilter,
) -> ChildProcessIdMap {
    let mut map = ChildProcessIdMap::new();
    for process_info in process_infos {
        if filter(process_info) {
            continue;
        }
        let children = map.entry(process_info.parent_process_id).or_default();
        children.push(process_info.process_id);
    }
    for (_, children) in &mut map.iter_mut() {
        children.sort_unstable();
    }
    map
}

/// Create a map from process id to process info.
pub(crate) fn get_process_info_map(process_infos: ProcessInfos) -> ProcessInfoMap {
    let mut map = ProcessInfoMap::new();
    for process_info in process_infos {
        map.insert(process_info.process_id, process_info);
    }
    map
}

/// Breadth-first search to get all process ids to kill.
pub(crate) fn get_process_ids_to_kill(
    target_process_id: ProcessId,
    child_process_id_map: &ChildProcessIdMap,
    config: &Config,
) -> ProcessIds {
    let mut process_ids_to_kill = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(target_process_id);
    while let Some(process_id) = queue.pop_front() {
        if process_id == target_process_id {
            if config.include_target {
                process_ids_to_kill.push(process_id);
            } else {
                debug!(
                    process_id,
                    include_target = config.include_target,
                    "Skipping target process id"
                );
            }
        } else {
            process_ids_to_kill.push(process_id);
        }
        if let Some(children) = child_process_id_map.get(&process_id) {
            for &child in children {
                queue.push_back(child);
            }
        }
    }
    process_ids_to_kill
}

pub(crate) fn parse_kill_output(
    kill_output: KillOutput,
    process_info_map: &mut ProcessInfoMap,
) -> Option<Output> {
    match kill_output {
        KillOutput::Killed { process_id } => {
            let Some(process_info) = process_info_map.remove(&process_id) else {
                debug!(process_id, "Process info not found");
                return None;
            };

            Some(Output::Killed {
                process_id: process_info.process_id,
                parent_process_id: process_info.parent_process_id,
                name: process_info.name,
            })
        }
        KillOutput::MaybeAlreadyTerminated { process_id, source } => {
            Some(Output::MaybeAlreadyTerminated { process_id, source })
        }
    }
}

pub(crate) fn kill_tree_internal(
    process_id: ProcessId,
    config: &Config,
    process_infos: ProcessInfos,
) -> Result<Outputs> {
    let child_process_id_map =
        crate::common::get_child_process_id_map(&process_infos, imp::child_process_id_map_filter);
    let process_ids_to_kill =
        crate::common::get_process_ids_to_kill(process_id, &child_process_id_map, config);
    let killer = imp::new_killer(config)?;
    let mut outputs = Outputs::new();
    let mut process_info_map = crate::common::get_process_info_map(process_infos);
    // kill children first
    for &process_id in process_ids_to_kill.iter().rev() {
        let kill_output = killer.kill(process_id)?;
        let Some(output) = crate::common::parse_kill_output(kill_output, &mut process_info_map)
        else {
            continue;
        };
        outputs.push(output);
    }
    Ok(outputs)
}
