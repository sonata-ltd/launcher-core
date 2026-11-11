pub mod coordinate;
pub mod legacy;

pub use coordinate::{MavenCoordinate, MavenCoordinateError};
pub use legacy::build_file_path;
