use crate::instance::content::model::DependencyKind;

pub fn dependency_kind(relation_type: u32) -> DependencyKind {
    match relation_type {
        1 => DependencyKind::Embedded,
        2 => DependencyKind::Optional,
        3 => DependencyKind::Required,
        4 => DependencyKind::Tool,
        5 => DependencyKind::Incompatible,
        6 => DependencyKind::Include,
        value => DependencyKind::Unknown(value.to_string()),
    }
}
