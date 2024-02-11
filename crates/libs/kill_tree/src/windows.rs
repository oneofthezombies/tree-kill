use crate::core::{Config, Error, KillOutput, ProcessId, ProcessInfo, ProcessInfos, Result};
use std::ffi;
use tracing::instrument;
use windows::Win32::{
    Foundation::{CloseHandle, ERROR_NO_MORE_FILES, E_ACCESSDENIED, E_INVALIDARG},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32First, Process32Next, PROCESSENTRY32,
            TH32CS_SNAPPROCESS,
        },
        Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE},
    },
};

impl From<windows::core::Error> for Error {
    fn from(error: windows::core::Error) -> Self {
        Self::Windows(error)
    }
}

/// process id of System Idle Process
const SYSTEM_IDLE_PROCESS_PROCESS_ID: u32 = 0;

/// process id of System
const SYSTEM_PROCESS_ID: u32 = 4;

pub(crate) fn validate_process_id(process_id: ProcessId) -> Result<()> {
    match process_id {
        SYSTEM_IDLE_PROCESS_PROCESS_ID => Err(Error::InvalidProcessId {
            process_id,
            reason: "Not allowed to kill System Idle Process".into(),
        }),
        SYSTEM_PROCESS_ID => Err(Error::InvalidProcessId {
            process_id,
            reason: "Not allowed to kill System".into(),
        }),
        _ => Ok(()),
    }
}

pub(crate) fn child_process_id_map_filter(process_info: &ProcessInfo) -> bool {
    // this process is System Idle Process
    process_info.parent_process_id == process_info.process_id
}

#[cfg(feature = "blocking")]
pub(crate) mod blocking {
    use super::*;

    struct Killer {}

    impl crate::blocking::Killable for Killer {
        fn kill(&self, process_id: ProcessId) -> Result<KillOutput> {
            crate::windows::blocking::kill(process_id)
        }
    }

    pub(crate) fn new_killer(_config: &Config) -> Result<impl crate::blocking::Killable> {
        Ok(Killer {})
    }

    #[instrument]
    pub(crate) fn kill(process_id: ProcessId) -> Result<KillOutput> {
        let result: Result<KillOutput>;
        unsafe {
            let open_result = OpenProcess(PROCESS_TERMINATE, false, process_id);
            match open_result {
                Ok(process_handle) => {
                    {
                        // do NOT return early from this block
                        result = TerminateProcess(process_handle, 1)
                            .and(Ok(KillOutput::Killed { process_id }))
                            .or_else(|e| {
                                if e.code() == E_ACCESSDENIED {
                                    // Access is denied.
                                    // This happens when the process is already terminated.
                                    // This treat as success.
                                    Ok(KillOutput::MaybeAlreadyTerminated {
                                        process_id,
                                        source: e.into(),
                                    })
                                } else {
                                    Err(e.into())
                                }
                            });
                    }
                    CloseHandle(process_handle)?;
                }
                Err(e) => {
                    if e.code() == E_INVALIDARG {
                        // The parameter is incorrect.
                        // This happens when the process is already terminated.
                        // This treat as success.
                        result = Ok(KillOutput::MaybeAlreadyTerminated {
                            process_id,
                            source: e.into(),
                        });
                    } else {
                        result = Err(e.into());
                    }
                }
            }
        }
        result
    }

    #[instrument]
    pub(crate) fn get_process_infos() -> Result<ProcessInfos> {
        let mut process_infos = ProcessInfos::new();
        let mut error: Option<Error> = None;
        unsafe {
            let snapshot_handle = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?;
            {
                // do NOT return early from this block
                let mut process_entry = std::mem::zeroed::<PROCESSENTRY32>();
                let process_entry_size = u32::try_from(std::mem::size_of::<PROCESSENTRY32>());
                match process_entry_size {
                    Ok(process_entry_size) => {
                        process_entry.dwSize = process_entry_size;
                        match Process32First(snapshot_handle, &mut process_entry) {
                            Ok(()) => loop {
                                process_infos.push(ProcessInfo {
                                    process_id: process_entry.th32ProcessID,
                                    parent_process_id: process_entry.th32ParentProcessID,
                                    name: ffi::CStr::from_ptr(
                                        process_entry.szExeFile.as_ptr().cast(),
                                    )
                                    .to_string_lossy()
                                    .into_owned(),
                                });
                                match Process32Next(snapshot_handle, &mut process_entry) {
                                    Ok(()) => {}
                                    Err(e) => {
                                        if e.code() != ERROR_NO_MORE_FILES.into() {
                                            error = Some(e.into());
                                        }
                                        break;
                                    }
                                }
                            },
                            Err(e) => {
                                error = Some(e.into());
                            }
                        }
                    }
                    Err(e) => {
                        error = Some(Error::InvalidCast {
                            source: e,
                            reason: "size of PROCESSENTRY32 to u32".into(),
                        });
                    }
                }
            }
            CloseHandle(snapshot_handle)?;
        }
        if let Some(e) = error {
            Err(e)
        } else {
            Ok(process_infos)
        }
    }
}

#[cfg(feature = "tokio")]
pub(crate) mod tokio {
    use super::*;
    use async_trait::async_trait;

    #[derive(Clone)]
    struct Killer {}

    #[async_trait]
    impl crate::tokio::Killable for Killer {
        async fn kill(&self, process_id: ProcessId) -> Result<KillOutput> {
            crate::windows::tokio::kill(process_id).await
        }
    }

    pub(crate) fn new_killer(_config: &Config) -> Result<impl crate::tokio::Killable> {
        Ok(Killer {})
    }

    #[instrument]
    async fn kill(process_id: ProcessId) -> Result<KillOutput> {
        ::tokio::task::spawn_blocking(move || crate::windows::blocking::kill(process_id)).await?
    }

    #[instrument]
    pub(crate) async fn get_process_infos() -> Result<ProcessInfos> {
        ::tokio::task::spawn_blocking(move || blocking::get_process_infos()).await?
    }
}

// #[cfg(test)]
// mod tests {
//     #[tokio::test]
//     async fn process_id_0() {
//         let result = kill_tree(0).await;
//         assert!(result.is_err());
//         assert_eq!(
//             result.unwrap_err().to_string(),
//             "Not allowed to kill System Idle Process. process id: 0"
//         );
//     }

//     #[tokio::test]
//     async fn process_id_4() {
//         let result = kill_tree(4).await;
//         assert!(result.is_err());
//         assert_eq!(
//             result.unwrap_err().to_string(),
//             "Not allowed to kill System. process id: 4"
//         );
//     }
// }
