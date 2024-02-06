use crate::{
    common::{self, Impl, ProcessInfo, ProcessInfos},
    ProcessId,
};
use std::{
    ffi::{c_void, CStr},
    io, ptr,
};
use tokio::task::JoinSet;
use tracing::{debug, instrument};

const AVAILABLE_MAX_PROCESS_ID: u32 = 99999 - 1;

#[instrument]
pub(crate) async fn get_process_info(process_id: ProcessId) -> Option<ProcessInfo> {
    let proc_bsdinfo_size = match u32::try_from(std::mem::size_of::<libproc::proc_bsdinfo>()) {
        Ok(x) => x,
        Err(e) => {
            debug!(error = ?e, "failed to convert size of proc_bsdinfo");
            return None;
        }
    };
    let proc_bsdinfo_size_sign = match i32::try_from(proc_bsdinfo_size) {
        Ok(x) => x,
        Err(e) => {
            debug!(error = ?e, "failed to convert size of proc_bsdinfo");
            return None;
        }
    };
    let mut proc_bsdinfo = unsafe { std::mem::zeroed::<libproc::proc_bsdinfo>() };
    let proc_pidtbsdinfo_sign = match i32::try_from(libproc::PROC_PIDTBSDINFO) {
        Ok(x) => x,
        Err(e) => {
            debug!(error = ?e, "failed to convert PROC_PIDTBSDINFO");
            return None;
        }
    };
    let process_id_sign = match i32::try_from(process_id) {
        Ok(x) => x,
        Err(e) => {
            debug!(error = ?e, process_id, "failed to convert process id");
            return None;
        }
    };
    let result = unsafe {
        libproc::proc_pidinfo(
            process_id_sign,
            proc_pidtbsdinfo_sign,
            0,
            std::ptr::addr_of_mut!(proc_bsdinfo).cast::<c_void>(),
            proc_bsdinfo_size_sign,
        )
    };
    if result <= 0 {
        let error = io::Error::last_os_error();
        debug!(error = ?error, process_id, "failed to get process info");
        return None;
    }
    let name = unsafe { CStr::from_ptr(std::ptr::addr_of!(proc_bsdinfo.pbi_name[0])) }
        .to_string_lossy()
        .to_string();
    Some(ProcessInfo {
        process_id,
        parent_process_id: proc_bsdinfo.pbi_ppid,
        name,
    })
}

#[instrument]
pub(crate) async fn get_process_infos() -> common::Result<ProcessInfos> {
    let buffer_size_sign =
        unsafe { libproc::proc_listpids(libproc::PROC_ALL_PIDS, 0_u32, ptr::null_mut(), 0) };
    if buffer_size_sign <= 0 {
        return Err(io::Error::last_os_error().into());
    }
    let buffer_size = match usize::try_from(buffer_size_sign) {
        Ok(x) => x,
        Err(e) => {
            debug!(error = ?e, "failed to convert buffer size");
            return Err(e.into());
        }
    };
    let mut buffer = vec![0; buffer_size];
    let result = unsafe {
        libproc::proc_listpids(
            libproc::PROC_ALL_PIDS,
            0_u32,
            buffer.as_mut_ptr().cast(),
            buffer_size_sign,
        )
    };
    if result <= 0 {
        return Err(io::Error::last_os_error().into());
    }
    let process_ids = buffer.as_slice();
    let mut tasks: JoinSet<Option<ProcessInfo>> = JoinSet::new();
    for &process_id_sign in process_ids {
        let process_id = match u32::try_from(process_id_sign) {
            Ok(x) => x,
            Err(e) => {
                debug!(error = ?e, "failed to convert process id");
                continue;
            }
        };
        tasks.spawn(get_process_info(process_id));
    }
    let mut process_infos = ProcessInfos::new();
    while let Some(result) = tasks.join_next().await {
        let process_info = match result {
            Ok(x) => x,
            Err(e) => {
                debug!(error = ?e, "failed to get process info");
                continue;
            }
        };
        if let Some(process_info) = process_info {
            process_infos.push(process_info);
        }
    }
    Ok(process_infos)
}

impl Impl {
    pub(crate) fn validate_process_id(&self) -> common::Result<()> {
        crate::unix::validate_process_id(self.process_id, AVAILABLE_MAX_PROCESS_ID)
    }

    pub(crate) async fn get_process_infos(&self) -> common::Result<ProcessInfos> {
        get_process_infos().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kill_tree;

    #[tokio::test]
    async fn process_id_max_plus_1() {
        let result = kill_tree(AVAILABLE_MAX_PROCESS_ID + 1).await;
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Process id is too large. process id: 99999, available max process id: 99998"
        );
    }
}

#[allow(warnings)]
#[allow(clippy::all)]
#[allow(clippy::pedantic)]
mod libproc {
    include!(concat!(env!("OUT_DIR"), "/libproc_bindings.rs"));
}
