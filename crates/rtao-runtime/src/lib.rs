//! Name-based Bittensor chain descriptors and typed runtime models.
#![deny(missing_docs)]

mod descriptor;
/// Generated chain descriptors, refreshed by `cargo xtask regen`.
pub mod generated;
mod handle;
/// Handwritten by-name runtime return models.
pub mod models;
/// Exact and projected structural metadata hashes.
pub mod shape;
mod surface;

pub use descriptor::{
    ApiSurface, Call, CallDescriptor, ConstantDescriptor, ConstantSurface, EventDescriptor,
    ExtensionDescriptor, FieldDescriptor, ModelFieldDescriptor, ModelFieldSurface, ModelSurface,
    ModuleError, NestedCalls, OptionalStorage, PalletDescriptor, ProjectionFactory,
    RuntimeApiDescriptor, StorageDescriptor, StorageQuery, StorageSurface, SurfaceDescriptor,
    nested_call_stamps, shape_parts,
};
pub use generated::selective::SelectiveMetagraphIndex;
pub use generated::{
    calls, constants, errors, events, extensions, runtime_apis, selective, shapes, storage,
};
pub use handle::{
    CallBytes, ComposeError, MetadataLevel, RuntimeEntry, RuntimeHandle, RuntimeStamp,
};
pub use shape::{
    OuterEnums, Projection, ProjectionField, ProjectionFields, ProjectionVariant, ShapeError,
    ShapeHash, ShapeHasher,
};
pub use surface::{
    SurfaceChange, SurfaceClass, SurfaceKind, SurfaceReport, verify_surface, verify_surface_with,
};
