pub mod coordinate;
pub mod legacy;

pub use coordinate::{MavenCoordinate, MavenCoordinateError};
#[allow(deprecated)]
pub use legacy::build_file_path;
