//! Type registry: script-visible component and struct layouts (ROADMAP 3.4,
//! `crates/efescript/docs/e3-design.md` section 9).
//!
//! Every registered type has a stable name, a numeric id (`fnv1a32(name)`), and its byte layout
//! as an [`efevm::StructDesc`]. Component types (registered with [`TypeRegistry::register`])
//! also get byte-level read / write / insert / remove functions, so code that only knows the id
//! and a byte buffer (scripts, scenes) can work with typed hecs components. Layout-only types
//! ([`TypeRegistry::register_struct`], e.g. from a mod) have no hecs component behind them.

use std::any::TypeId;
use std::collections::HashMap;

use efevm::{Prim, StructDesc};
use hecs::{Component, ComponentError, ComponentId, DynRef, DynRefMut, DynamicError, Entity, World};

/// A `#[repr(C)]`, `Pod` type with a described byte layout. Derive it with
/// `#[derive(ScriptType)]`; the derive fills [`ScriptType::layout`] from `offset_of!`.
///
/// ```
/// use efeengine_core::prelude::*;
/// /// Hit points.
/// #[repr(C)]
/// #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable, ScriptType)]
/// struct Health { current: f32, max: f32 }
/// assert_eq!(Health::layout().fields[1].offset, 4);
/// ```
///
/// The derive rejects input it cannot describe:
/// ```compile_fail
/// use efeengine_core::prelude::*;
/// #[repr(C)]
/// #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable, ScriptType)]
/// struct Tuple(f32, f32);
/// ```
/// ```compile_fail
/// use efeengine_core::prelude::*;
/// #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable, ScriptType)]
/// struct NoRepr { a: f32 }
/// ```
/// ```compile_fail
/// use efeengine_core::prelude::*;
/// #[repr(C)]
/// #[derive(Clone, Copy, ScriptType)]
/// struct Generic<T> { a: T }
/// ```
/// ```compile_fail
/// use efeengine_core::prelude::*;
/// #[repr(C)]
/// #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable, ScriptType)]
/// struct BadEntity { #[script(entity)] e: u32 }
/// ```
pub trait ScriptType: bytemuck::Pod {
    /// The stable name (a plain identifier). Ids are derived from it.
    const NAME: &'static str;
    /// The byte layout (owner and component id are filled by the registry).
    fn layout() -> StructDesc;
}

/// A field type the `ScriptType` derive understands: `COUNT` consecutive values of `PRIM`.
pub trait ScriptField {
    const PRIM: Prim;
    const COUNT: u32;
}

macro_rules! scalar_fields {
    ($($t:ty => $p:ident),* $(,)?) => {
        $(impl ScriptField for $t { const PRIM: Prim = Prim::$p; const COUNT: u32 = 1; })*
    };
}
scalar_fields!(u8 => U8, i8 => I8, u16 => U16, i16 => I16, u32 => U32, i32 => I32, u64 => U64, i64 => I64, f32 => F32, f64 => F64);

impl<T: ScriptField, const N: usize> ScriptField for [T; N] {
    const PRIM: Prim = T::PRIM;
    const COUNT: u32 = T::COUNT * N as u32;
}

macro_rules! vector_fields {
    ($($t:ty => $p:ident x $n:literal),* $(,)?) => {
        $(impl ScriptField for $t { const PRIM: Prim = Prim::$p; const COUNT: u32 = $n; })*
    };
}
vector_fields!(
    glam::Vec2 => F32 x 2, glam::Vec3 => F32 x 3, glam::Vec4 => F32 x 4,
    glam::IVec2 => I32 x 2, glam::IVec3 => I32 x 3, glam::IVec4 => I32 x 4,
    glam::UVec2 => U32 x 2, glam::UVec3 => U32 x 3, glam::UVec4 => U32 x 4,
);

/// Error of a byte-level component operation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompError {
    /// The entity does not exist (despawned, stale generation or forged bits).
    #[error("no such entity")]
    NoEntity,
    /// The entity exists but has no component of this type.
    #[error("the entity has no `{0}` component")]
    Missing(String),
    /// The byte buffer does not have the component's size.
    #[error("`{name}` is {expected} bytes, got {got}")]
    WrongSize { name: String, expected: u32, got: usize },
    /// No registered type has this id.
    #[error("unknown type id {0:#010x}")]
    UnknownId(u32),
    /// The type is a layout-only struct, not a component.
    #[error("`{0}` is not a component")]
    NotComponent(String),
    /// A mod-defined component that is not (or no longer) registered in the world.
    #[error("component `{0}` is not registered in the world")]
    Unregistered(String),
    /// The type has no byte-level access (a Rust component read through [`TypeEntry::dyn_get`]).
    #[error("`{0}` is not a mod-defined component")]
    NotDynamic(String),
    /// A named field that the layout does not have, or has with another type.
    #[error("`{component}` has no field `{field}` of that type")]
    NoField { component: String, field: String },
}

/// Error of a registration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TypeError {
    #[error("a type named `{0}` is already registered")]
    DuplicateName(String),
    #[error("the Rust type of `{0}` is already registered")]
    DuplicateType(String),
    #[error("`{name}` and `{other}` have the same id {id:#010x}")]
    IdCollision { name: String, other: String, id: u32 },
    #[error("invalid layout: {0}")]
    Invalid(String),
    #[error("cannot register {what}: API registration is closed once the game loop starts; register in setup or Plugin::build (mods declare theirs in mod.toml)")]
    Closed { what: String },
}

type ReadFn = fn(&World, Entity, &mut [u8]) -> Result<(), CompError>;
type WriteFn = fn(&mut World, Entity, &[u8]) -> Result<(), CompError>;
type RemoveFn = fn(&mut World, Entity) -> Result<(), CompError>;
type HasFn = fn(&World, Entity) -> bool;

#[derive(Clone, Copy)]
struct ComponentFns {
    type_id: TypeId,
    read: ReadFn,
    write: WriteFn,
    insert: WriteFn,
    remove: RemoveFn,
    has: HasFn,
}

/// How a registered type is stored.
#[derive(Clone, Copy)]
enum Storage {
    /// A Rust component type with typed byte-level functions.
    Rust(ComponentFns),
    /// A mod-defined component: plain bytes in the world's dynamic columns
    /// ([`World::register_dynamic`]).
    Dynamic,
    /// A layout-only struct: no component behind it.
    Layout,
}

/// One registered type.
#[derive(Clone)]
pub struct TypeEntry {
    id: u32,
    desc: StructDesc,
    storage: Storage,
}

impl std::fmt::Debug for TypeEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypeEntry").field("id", &self.id).field("desc", &self.desc).field("component", &self.is_component()).finish()
    }
}

fn dyn_err(name: &str, e: DynamicError) -> CompError {
    match e {
        DynamicError::NoSuchEntity => CompError::NoEntity,
        DynamicError::Missing => CompError::Missing(name.to_string()),
        DynamicError::Unregistered(_) => CompError::Unregistered(name.to_string()),
        DynamicError::WrongSize { expected, got } => CompError::WrongSize { name: name.to_string(), expected: expected as u32, got },
        DynamicError::AlreadyRegistered(_) | DynamicError::BadLayout => CompError::Unregistered(name.to_string()),
    }
}

impl TypeEntry {
    pub fn id(&self) -> u32 {
        self.id
    }
    pub fn name(&self) -> &str {
        &self.desc.name
    }
    /// The layout; `desc().component` is `Some(id)` for components.
    pub fn desc(&self) -> &StructDesc {
        &self.desc
    }
    pub fn size(&self) -> u32 {
        self.desc.size
    }
    pub fn align(&self) -> u32 {
        self.desc.align
    }
    /// `true` for a component (a Rust type or a mod-defined one), `false` for a layout-only
    /// struct.
    pub fn is_component(&self) -> bool {
        !matches!(self.storage, Storage::Layout)
    }
    /// `true` for a mod-defined component (bytes in the world's dynamic columns).
    pub fn is_dynamic(&self) -> bool {
        matches!(self.storage, Storage::Dynamic)
    }
    /// The Rust type of a component (`None` for mod-defined components and layout-only structs).
    pub fn type_id(&self) -> Option<TypeId> {
        match &self.storage {
            Storage::Rust(f) => Some(f.type_id),
            _ => None,
        }
    }

    /// The efecs [`ComponentId`] of a component (`None` for a layout-only struct, and for a
    /// mod-defined component that is not registered in `world`). Takes the world because
    /// dynamic components (registered at run time) are looked up there; the id carries the
    /// generation of the registration.
    pub fn component_id(&self, world: &World) -> Option<ComponentId> {
        match &self.storage {
            Storage::Rust(f) => Some(ComponentId::from(f.type_id)),
            Storage::Dynamic => world.dynamic_info(self.id).map(|(c, _)| c),
            Storage::Layout => None,
        }
    }

    fn not_component(&self) -> CompError {
        CompError::NotComponent(self.desc.name.clone())
    }

    /// Copy the component of `e` into `out` (`out.len()` must be the size).
    pub fn read(&self, world: &World, e: Entity, out: &mut [u8]) -> Result<(), CompError> {
        match &self.storage {
            Storage::Rust(f) => (f.read)(world, e, out),
            Storage::Dynamic => {
                let r = world.get_dynamic(e, self.id).map_err(|x| dyn_err(&self.desc.name, x))?;
                if out.len() != r.len() {
                    return Err(CompError::WrongSize { name: self.desc.name.clone(), expected: r.len() as u32, got: out.len() });
                }
                out.copy_from_slice(&r);
                Ok(())
            }
            Storage::Layout => Err(self.not_component()),
        }
    }
    /// Overwrite the existing component of `e` with `bytes`. A mod-defined component that
    /// tracks changes is stamped changed.
    pub fn write(&self, world: &mut World, e: Entity, bytes: &[u8]) -> Result<(), CompError> {
        match &self.storage {
            Storage::Rust(f) => (f.write)(world, e, bytes),
            Storage::Dynamic => {
                let mut r = world.get_dynamic_mut(e, self.id).map_err(|x| dyn_err(&self.desc.name, x))?;
                if bytes.len() != r.len() {
                    return Err(CompError::WrongSize { name: self.desc.name.clone(), expected: r.len() as u32, got: bytes.len() });
                }
                r.copy_from_slice(bytes);
                Ok(())
            }
            Storage::Layout => Err(self.not_component()),
        }
    }
    /// Add (or replace) the component of `e` from `bytes`.
    pub fn insert(&self, world: &mut World, e: Entity, bytes: &[u8]) -> Result<(), CompError> {
        match &self.storage {
            Storage::Rust(f) => (f.insert)(world, e, bytes),
            Storage::Dynamic => world.insert_dynamic(e, self.id, bytes).map_err(|x| dyn_err(&self.desc.name, x)),
            Storage::Layout => Err(self.not_component()),
        }
    }
    /// Remove the component from `e`.
    pub fn remove(&self, world: &mut World, e: Entity) -> Result<(), CompError> {
        match &self.storage {
            Storage::Rust(f) => (f.remove)(world, e),
            Storage::Dynamic => world.remove_dynamic(e, self.id).map_err(|x| dyn_err(&self.desc.name, x)),
            Storage::Layout => Err(self.not_component()),
        }
    }
    /// `true` if `e` is alive and has the component (`false` for layout-only types).
    pub fn has(&self, world: &World, e: Entity) -> bool {
        match &self.storage {
            Storage::Rust(f) => (f.has)(world, e),
            Storage::Dynamic => world.has_dynamic(e, self.id),
            Storage::Layout => false,
        }
    }

    /// Shared borrow of the bytes of a mod-defined component (holds the column borrow).
    pub fn dyn_ref<'w>(&self, world: &'w World, e: Entity) -> Result<DynRef<'w>, CompError> {
        if !self.is_dynamic() {
            return Err(if self.is_component() { CompError::NotDynamic(self.desc.name.clone()) } else { self.not_component() });
        }
        world.get_dynamic(e, self.id).map_err(|x| dyn_err(&self.desc.name, x))
    }

    /// Exclusive-by-contract borrow of the bytes of a mod-defined component; stamps it changed.
    pub fn dyn_ref_mut<'w>(&self, world: &'w World, e: Entity) -> Result<DynRefMut<'w>, CompError> {
        if !self.is_dynamic() {
            return Err(if self.is_component() { CompError::NotDynamic(self.desc.name.clone()) } else { self.not_component() });
        }
        world.get_dynamic_mut(e, self.id).map_err(|x| dyn_err(&self.desc.name, x))
    }
}

/// A read-only typed view of a mod-defined component: its layout and its bytes. Hold it only
/// briefly; the bytes borrow the world's column.
pub struct DynComponent<'a> {
    pub desc: &'a StructDesc,
    pub bytes: &'a [u8],
}

fn field_of<'d>(desc: &'d StructDesc, name: &str, size: usize) -> Option<&'d efevm::FieldDesc> {
    desc.field_named(name).filter(|f| f.ty.size() as usize * f.count as usize == size)
}

impl<'a> DynComponent<'a> {
    /// The field `name` as `T`. `T` must have the size of the whole field (`[f32; 2]` reads an
    /// `f32` field with `count = 2`); the primitive type is not checked beyond the size.
    pub fn field<T: bytemuck::Pod>(&self, name: &str) -> Option<T> {
        let f = field_of(self.desc, name, size_of::<T>())?;
        let at = f.offset as usize;
        Some(bytemuck::pod_read_unaligned(self.bytes.get(at..at + size_of::<T>())?))
    }
    /// A scalar `f32` field.
    pub fn field_f32(&self, name: &str) -> Option<f32> {
        self.scalar(name, Prim::F32).and_then(|_| self.field::<f32>(name))
    }
    /// A scalar `i32` field.
    pub fn field_i32(&self, name: &str) -> Option<i32> {
        self.scalar(name, Prim::I32).and_then(|_| self.field::<i32>(name))
    }
    /// A scalar `u32` field.
    pub fn field_u32(&self, name: &str) -> Option<u32> {
        self.scalar(name, Prim::U32).and_then(|_| self.field::<u32>(name))
    }
    fn scalar(&self, name: &str, ty: Prim) -> Option<()> {
        self.desc.field_named(name).filter(|f| f.ty == ty && f.count == 1).map(|_| ())
    }
}

/// The borrow behind [`TypeRegistry::view`]: the layout and the column guard.
pub struct DynView<'w> {
    desc: StructDesc,
    guard: DynRef<'w>,
}

impl DynView<'_> {
    /// The typed view of the bytes.
    pub fn get(&self) -> DynComponent<'_> {
        DynComponent { desc: &self.desc, bytes: &self.guard }
    }
    pub fn field<T: bytemuck::Pod>(&self, name: &str) -> Option<T> {
        self.get().field(name)
    }
    pub fn field_f32(&self, name: &str) -> Option<f32> {
        self.get().field_f32(name)
    }
}

/// The borrow behind [`TypeRegistry::view_mut`].
pub struct DynViewMut<'w> {
    desc: StructDesc,
    guard: DynRefMut<'w>,
}

impl DynViewMut<'_> {
    /// The typed mutable view of the bytes.
    pub fn get_mut(&mut self) -> DynComponentMut<'_> {
        DynComponentMut { desc: &self.desc, bytes: &mut self.guard }
    }
    pub fn set_field<T: bytemuck::Pod>(&mut self, name: &str, value: T) -> bool {
        self.get_mut().set_field(name, value)
    }
}

/// A mutable typed view of a mod-defined component (see [`DynComponent`]).
pub struct DynComponentMut<'a> {
    pub desc: &'a StructDesc,
    pub bytes: &'a mut [u8],
}

impl<'a> DynComponentMut<'a> {
    /// Write `value` into field `name` (`T` must have the size of the whole field). `false` if
    /// there is no such field or the size differs.
    pub fn set_field<T: bytemuck::Pod>(&mut self, name: &str, value: T) -> bool {
        let Some(f) = field_of(self.desc, name, size_of::<T>()) else { return false };
        let at = f.offset as usize;
        match self.bytes.get_mut(at..at + size_of::<T>()) {
            Some(dst) => {
                dst.copy_from_slice(bytemuck::bytes_of(&value));
                true
            }
            None => false,
        }
    }
    /// Read-only access to the same bytes.
    pub fn as_view(&self) -> DynComponent<'_> {
        DynComponent { desc: self.desc, bytes: self.bytes }
    }
}

fn comp_err<T: ScriptType>(e: ComponentError) -> CompError {
    match e {
        ComponentError::NoSuchEntity => CompError::NoEntity,
        ComponentError::MissingComponent(_) => CompError::Missing(T::NAME.to_string()),
    }
}

fn check_len<T: ScriptType>(len: usize) -> Result<(), CompError> {
    if len == size_of::<T>() {
        Ok(())
    } else {
        Err(CompError::WrongSize { name: T::NAME.to_string(), expected: size_of::<T>() as u32, got: len })
    }
}

fn read_impl<T: ScriptType + Component>(world: &World, e: Entity, out: &mut [u8]) -> Result<(), CompError> {
    check_len::<T>(out.len())?;
    let c = world.get::<&T>(e).map_err(comp_err::<T>)?;
    out.copy_from_slice(bytemuck::bytes_of(&*c));
    Ok(())
}

fn write_impl<T: ScriptType + Component>(world: &mut World, e: Entity, bytes: &[u8]) -> Result<(), CompError> {
    check_len::<T>(bytes.len())?;
    let v: T = bytemuck::pod_read_unaligned(bytes);
    let mut c = world.get::<&mut T>(e).map_err(comp_err::<T>)?;
    *c = v;
    Ok(())
}

fn insert_impl<T: ScriptType + Component>(world: &mut World, e: Entity, bytes: &[u8]) -> Result<(), CompError> {
    check_len::<T>(bytes.len())?;
    let v: T = bytemuck::pod_read_unaligned(bytes);
    world.insert_one(e, v).map_err(|_| CompError::NoEntity)
}

fn remove_impl<T: ScriptType + Component>(world: &mut World, e: Entity) -> Result<(), CompError> {
    world.remove_one::<T>(e).map(drop).map_err(comp_err::<T>)
}

fn has_impl<T: Component>(world: &World, e: Entity) -> bool {
    world.satisfies::<&T>(e)
}

/// The owner recorded for types registered without one.
pub const ENGINE_OWNER: &str = "engine";

/// All script-visible types: components and layout-only structs, by id and by name.
///
/// Registering is `pub(crate)`: the only public way in is `Engine::register_component` /
/// `Engine::register_struct`, which also put the layout in the API index (the scripting
/// host registry). So every component id a script can pass is in the index.
///
/// ```compile_fail
/// use efeengine_core::prelude::*;
/// let mut types = TypeRegistry::new();
/// let _ = types.register::<Transform>(); // private: use `Engine::register_component`
/// ```
#[derive(Debug, Default, Clone)]
pub struct TypeRegistry {
    entries: Vec<TypeEntry>,
    by_id: HashMap<u32, usize>,
    by_name: HashMap<String, usize>,
    /// Bumped on every change; lets the scripting host see that its copy is stale.
    generation: u64,
}

impl TypeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The id a type with this name gets: `fnv1a32(name)`.
    pub fn id_of_name(name: &str) -> u32 {
        efevm::api::fnv1a32(name.as_bytes())
    }

    /// Register component type `T` with owner `engine`; returns its id.
    pub(crate) fn register<T: ScriptType + Component>(&mut self) -> Result<u32, TypeError> {
        self.register_owned::<T>(ENGINE_OWNER)
    }

    /// Register component type `T` with the given owner (`engine`, a game or a mod name).
    pub(crate) fn register_owned<T: ScriptType + Component>(&mut self, owner: &str) -> Result<u32, TypeError> {
        let type_id = TypeId::of::<T>();
        if let Some(other) = self.entries.iter().find(|e| e.type_id() == Some(type_id)) {
            return Err(TypeError::DuplicateType(other.desc.name.clone()));
        }
        let mut desc = T::layout();
        desc.name = T::NAME.to_string();
        desc.owner = owner.to_string();
        let fns = ComponentFns {
            type_id,
            read: read_impl::<T>,
            write: write_impl::<T>,
            insert: insert_impl::<T>,
            remove: remove_impl::<T>,
            has: has_impl::<T>,
        };
        self.add(desc, Storage::Rust(fns))
    }

    /// Register a mod-defined component (bytes in the world's dynamic columns; the caller
    /// registers it in the world with [`World::register_dynamic`] under the returned id).
    /// `desc.name` is `mod:<mod>/<Name>`, `desc.owner` `mod:<mod>`. Returns the id.
    pub(crate) fn register_dynamic(&mut self, desc: StructDesc) -> Result<u32, TypeError> {
        if desc.component.is_some() {
            return Err(TypeError::Invalid(format!("`{}`: the component id is assigned by the registry", desc.name)));
        }
        self.add(desc, Storage::Dynamic)
    }

    /// Register a layout-only struct (no hecs component; e.g. defined by a mod at run time).
    /// `desc.component` must be `None`; an empty owner becomes `engine`. Returns its id.
    pub(crate) fn register_struct(&mut self, mut desc: StructDesc) -> Result<u32, TypeError> {
        if desc.component.is_some() {
            return Err(TypeError::Invalid(format!("`{}`: a runtime struct cannot be a component", desc.name)));
        }
        if desc.owner.is_empty() {
            desc.owner = ENGINE_OWNER.to_string();
        }
        self.add(desc, Storage::Layout)
    }

    fn add(&mut self, mut desc: StructDesc, storage: Storage) -> Result<u32, TypeError> {
        desc.validate().map_err(TypeError::Invalid)?;
        if self.by_name.contains_key(&desc.name) {
            return Err(TypeError::DuplicateName(desc.name));
        }
        let id = Self::id_of_name(&desc.name);
        if let Some(&i) = self.by_id.get(&id) {
            return Err(TypeError::IdCollision { name: desc.name, other: self.entries[i].desc.name.clone(), id });
        }
        if !matches!(storage, Storage::Layout) {
            desc.component = Some(id);
        }
        self.generation += 1;
        let i = self.entries.len();
        self.by_id.insert(id, i);
        self.by_name.insert(desc.name.clone(), i);
        self.entries.push(TypeEntry { id, desc, storage });
        Ok(id)
    }

    /// Remove every type owned by `owner` (a mod unload; keeps the registry in step with
    /// the API index, which drops them too). Returns how many were removed.
    ///
    /// The mod's components have the owner `mod:<owner>` and go with it.
    pub(crate) fn remove_owner(&mut self, owner: &str) -> usize {
        let before = self.entries.len();
        let comp_owner = format!("mod:{owner}");
        self.entries.retain(|e| e.desc.owner != owner && e.desc.owner != comp_owner);
        self.generation += 1;
        self.reindex();
        before - self.entries.len()
    }

    /// Remove the mod-defined components of mod `owner` only.
    pub(crate) fn remove_mod_components(&mut self, owner: &str) -> usize {
        let before = self.entries.len();
        let comp_owner = format!("mod:{owner}");
        self.entries.retain(|e| e.desc.owner != comp_owner);
        self.generation += 1;
        self.reindex();
        before - self.entries.len()
    }

    /// Remove the type with id `id` (used to roll back a registration that the scripting
    /// index refused). Returns `false` if there is no such type.
    pub(crate) fn unregister(&mut self, id: u32) -> bool {
        let Some(&i) = self.by_id.get(&id) else { return false };
        self.entries.remove(i);
        self.generation += 1;
        self.reindex();
        true
    }

    fn reindex(&mut self) {
        self.by_id.clear();
        self.by_name.clear();
        for (i, e) in self.entries.iter().enumerate() {
            self.by_id.insert(e.id, i);
            self.by_name.insert(e.desc.name.clone(), i);
        }
    }

    /// Changes with every registration or removal.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub fn get(&self, id: u32) -> Option<&TypeEntry> {
        self.by_id.get(&id).map(|&i| &self.entries[i])
    }

    pub fn by_name(&self, name: &str) -> Option<&TypeEntry> {
        self.by_name.get(name).map(|&i| &self.entries[i])
    }

    /// The entry of Rust component type `T`, if registered.
    pub fn of<T: 'static>(&self) -> Option<&TypeEntry> {
        let t = TypeId::of::<T>();
        self.entries.iter().find(|e| e.type_id() == Some(t))
    }

    /// The entry of a component id (`UnknownId` / `NotComponent` otherwise).
    pub fn component(&self, id: u32) -> Result<&TypeEntry, CompError> {
        let e = self.get(id).ok_or(CompError::UnknownId(id))?;
        if !e.is_component() {
            return Err(e.not_component());
        }
        Ok(e)
    }

    /// The mod-defined components owned by `owner` (a mod id) in registration order.
    pub fn mod_components<'a>(&'a self, owner: &'a str) -> impl Iterator<Item = &'a TypeEntry> + 'a {
        let comp_owner = format!("mod:{owner}");
        self.entries.iter().filter(move |e| e.is_dynamic() && e.desc.owner == comp_owner)
    }

    /// Borrow the mod-defined component `id` of `e` for reading its fields by name.
    pub fn view<'w>(&self, world: &'w World, e: Entity, id: u32) -> Result<DynView<'w>, CompError> {
        let entry = self.get(id).ok_or(CompError::UnknownId(id))?;
        let guard = entry.dyn_ref(world, e)?;
        Ok(DynView { desc: entry.desc.clone(), guard })
    }

    /// Like [`view`](Self::view), for writing (stamps the component changed).
    pub fn view_mut<'w>(&self, world: &'w World, e: Entity, id: u32) -> Result<DynViewMut<'w>, CompError> {
        let entry = self.get(id).ok_or(CompError::UnknownId(id))?;
        let guard = entry.dyn_ref_mut(world, e)?;
        Ok(DynViewMut { desc: entry.desc.clone(), guard })
    }

    /// All entries in registration order.
    pub fn entries(&self) -> &[TypeEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// An entity as it crosses a boundary (scripts, scenes, messages): `Entity::to_bits`.
pub fn entity_to_bits(e: Entity) -> u64 {
    e.to_bits().get()
}

/// Inverse of [`entity_to_bits`]; `None` for 0 and other bit patterns no entity can have.
/// A well-formed but stale handle decodes fine; world operations on it then fail cleanly
/// (`CompError::NoEntity`, or ignored by `Commands`).
pub fn entity_from_bits(bits: u64) -> Option<Entity> {
    Entity::from_bits(bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use efeengine_macros::ScriptType;

    /// A test struct.
    /// Second line.
    #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable, ScriptType)]
    #[repr(C)]
    #[script(name = "Target")]
    struct Tgt {
        /// Whom to follow.
        #[script(entity)]
        who: u64,
        weights: [f32; 3],
        cell: glam::IVec2,
        flags: u32,
    }

    #[test]
    fn derive_layout_docs_entity_and_name() {
        let d = Tgt::layout();
        assert_eq!(Tgt::NAME, "Target");
        assert_eq!(d.name, "Target");
        assert_eq!(d.doc, "A test struct.\nSecond line.");
        assert_eq!((d.size, d.align), (size_of::<Tgt>() as u32, align_of::<Tgt>() as u32));
        let who = d.field_named("who").unwrap();
        assert_eq!((who.ty, who.count, who.offset), (Prim::Entity, 1, 0));
        assert_eq!(who.doc, "Whom to follow.");
        let w = d.field_named("weights").unwrap();
        assert_eq!((w.ty, w.count, w.offset), (Prim::F32, 3, std::mem::offset_of!(Tgt, weights) as u32));
        let c = d.field_named("cell").unwrap();
        assert_eq!((c.ty, c.count, c.offset), (Prim::I32, 2, std::mem::offset_of!(Tgt, cell) as u32));
        assert_eq!(d.field_named("flags").unwrap().doc, "");
        d.validate().unwrap();
    }

    #[test]
    fn register_struct_and_errors() {
        let mut r = TypeRegistry::new();
        let id = r.register::<Tgt>().unwrap();
        assert_eq!(id, efevm::api::fnv1a32(b"Target"));
        assert_eq!(r.get(id).unwrap().desc().component, Some(id));
        assert_eq!(r.register::<Tgt>(), Err(TypeError::DuplicateType("Target".into())));
        // same name as a layout-only struct
        assert!(matches!(r.register_struct(StructDesc::new("Target", 4, 4)), Err(TypeError::DuplicateName(_))));
        // invalid layout
        assert!(matches!(r.register_struct(StructDesc::new("Bad", 6, 4)), Err(TypeError::Invalid(_))));
        let mut flagged = StructDesc::new("Flagged", 4, 4);
        flagged.component = Some(1);
        assert!(matches!(r.register_struct(flagged), Err(TypeError::Invalid(_))));
        let sid = r.register_struct(StructDesc::new("ModPoint", 8, 4).field(efevm::FieldDesc::new("x", 0, Prim::F32, 2))).unwrap();
        let e = r.get(sid).unwrap();
        assert!(!e.is_component());
        assert_eq!(e.desc().owner, "engine");
        assert_eq!(e.desc().component, None);
        assert_eq!(r.by_name("ModPoint").unwrap().id(), sid);
        assert_eq!(r.entries().len(), 2);
        // byte functions on a layout-only type
        let mut w = World::new();
        let ent = w.spawn(());
        assert_eq!(e.insert(&mut w, ent, &[0; 8]), Err(CompError::NotComponent("ModPoint".into())));
        assert!(!e.has(&w, ent));
        assert_eq!(r.component(sid).unwrap_err(), CompError::NotComponent("ModPoint".into()));
        assert_eq!(r.component(12345).unwrap_err(), CompError::UnknownId(12345));
    }

    #[test]
    fn id_collision_is_an_error() {
        // fnv1a32 collisions are rare; force one by injecting an entry with another name at the same id.
        let mut r = TypeRegistry::new();
        let id = r.register_struct(StructDesc::new("A", 4, 4)).unwrap();
        r.by_name.remove("A");
        r.entries[0].desc.name = "Z".into();
        r.by_name.insert("Z".into(), 0);
        match r.register_struct(StructDesc::new("A", 4, 4)) {
            Err(TypeError::IdCollision { name, other, id: cid }) => {
                assert_eq!((name.as_str(), other.as_str(), cid), ("A", "Z", id));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn entity_bits() {
        assert_eq!(entity_from_bits(0), None);
        let mut w = World::new();
        let e = w.spawn((1u32,));
        let bits = entity_to_bits(e);
        assert_eq!(entity_from_bits(bits), Some(e));
        w.despawn(e).unwrap();
        // well-formed but stale
        let stale = entity_from_bits(bits).unwrap();
        assert!(!w.contains(stale));
        // forged: a huge id that was never allocated
        if let Some(forged) = entity_from_bits(u64::MAX) {
            assert!(!w.contains(forged));
        }
    }
}
