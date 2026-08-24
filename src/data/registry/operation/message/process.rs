use crate::data::registry::operation::message::{
    status::ProgressUnit,
    target::{FileStatus, ProcessTarget},
};

impl ProcessTarget {
    pub fn file(name: String, status: FileStatus) -> Self {
        ProcessTarget::File {
            status: status.into(),
            name,
            unit: None,
            current: None,
            size: None,
        }
    }

    #[allow(dead_code)]
    pub fn file_with_details(
        name: String,
        status: FileStatus,
        unit: Option<ProgressUnit>,
        current: Option<usize>,
        size: Option<usize>,
    ) -> Self {
        ProcessTarget::File {
            status: status.into(),
            name,
            unit,
            current,
            size,
        }
    }

    // TODO for Dir
}
