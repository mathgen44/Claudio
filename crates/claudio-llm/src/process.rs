//! Processus enfants : sans fenêtre de console, et rattachés à un « job » Windows
//! pour qu'ils s'arrêtent avec Claudio, même en cas de plantage.

use std::sync::OnceLock;

pub fn hide_window(cmd: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

#[cfg(windows)]
mod job {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    pub struct Job(HANDLE);

    // SAFETY : un handle de job peut être utilisé depuis n'importe quel thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn create() -> windows::core::Result<Self> {
            // SAFETY : appels Win32 documentés ; le handle reste ouvert toute la vie
            // du programme et sa fermeture par le système tue les processus du job.
            unsafe {
                let h = CreateJobObjectW(None, None)?;
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )?;
                Ok(Self(h))
            }
        }

        pub fn assign(
            &self,
            process: std::os::windows::io::RawHandle,
        ) -> windows::core::Result<()> {
            unsafe { AssignProcessToJobObject(self.0, HANDLE(process)) }
        }
    }
}

/// Rattache le processus au job de Claudio (sans effet hors Windows).
pub fn attach(child: &tokio::process::Child) {
    #[cfg(windows)]
    {
        static JOB: OnceLock<Option<job::Job>> = OnceLock::new();
        let job = JOB.get_or_init(|| {
            job::Job::create()
                .map_err(|e| tracing::warn!("création du job Windows impossible : {e}"))
                .ok()
        });
        if let (Some(job), Some(h)) = (job, child.raw_handle())
            && let Err(e) = job.assign(h)
        {
            tracing::warn!("rattachement du processus au job impossible : {e}");
        }
    }
    #[cfg(not(windows))]
    let _ = (child, OnceLock::<()>::new());
}
