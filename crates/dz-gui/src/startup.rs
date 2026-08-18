use dz_core::errors::TaskError;
use dz_core::startup::{
    ContainerInstallTask, MachineInitTask, MachineStartTask, MachineStopOthersTask, Task,
    UpdateCheckTask, WSLInstallTask,
};
use dz_update::updater::Updater;

pub fn run_startup_tasks(mut logger: impl FnMut(String)) -> Result<(), TaskError> {
    let updater = Updater;
    let tasks: Vec<Box<dyn Task>> = vec![
        Box::new(WSLInstallTask),
        Box::new(MachineStopOthersTask),
        Box::new(MachineInitTask),
        Box::new(MachineStartTask),
        Box::new(UpdateCheckTask::new(Box::new(updater))),
        Box::new(ContainerInstallTask::new(Box::new(updater))),
    ];

    for task in tasks {
        if task.should_skip()? {
            logger(format!("Skipped {}", task.name()));
            continue;
        }
        logger(format!("Starting {}", task.name()));
        match task.run() {
            Ok(()) => logger(format!("Completed {}", task.name())),
            Err(error) => {
                task.handle_error(&error);
                if matches!(error, TaskError::UpdaterDisabledNoContainer(_)) {
                    logger(format!(
                        "{}: user chose not to install container image.",
                        task.name()
                    ));
                    return Ok(());
                }
                if task.can_fail() {
                    logger(format!("Warning: {} failed: {}", task.name(), error));
                    continue;
                }
                return Err(error);
            }
        }
    }
    Ok(())
}
