//! « Job » Windows propre à un processus : à sa fermeture (déconnexion, retrait du
//! serveur ou arrêt de Claudio), tout l'arbre de processus lancé est tué, y compris
//! les petits-enfants (`cmd /c npx …` → `node …`) que `kill_on_drop` laisserait.

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
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
        pub fn new(child: &tokio::process::Child) -> windows::core::Result<Self> {
            // SAFETY : appels Win32 documentés ; le handle est fermé par `Drop`.
            unsafe {
                let job = Self(CreateJobObjectW(None, None)?);
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )?;
                if let Some(h) = child.raw_handle() {
                    AssignProcessToJobObject(job.0, HANDLE(h))?;
                }
                Ok(job)
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY : handle obtenu de CreateJobObjectW, fermé une seule fois.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[cfg(windows)]
pub use imp::Job;

#[cfg(not(windows))]
pub struct Job;

#[cfg(not(windows))]
impl Job {
    pub fn new(_: &tokio::process::Child) -> std::io::Result<Self> {
        Ok(Self)
    }
}
