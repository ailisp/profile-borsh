use borsh::{BorshDeserialize, BorshSerialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Debug, BorshSerialize)]
pub struct CacheImage {
    /// The executable image.
    code: Vec<u8>,

    /// Offsets to the start of each function. Including trampoline, if any.
    /// Trampolines are only present on AArch64.
    /// On x86-64, `function_pointers` are identical to `function_offsets`.
    function_pointers: Vec<usize>,

    /// Offsets to the start of each function after trampoline.
    function_offsets: Vec<usize>,

    /// Number of imported functions.
    func_import_count: usize,

    /// Module state map.
    msm: ModuleStateMap,

    /// An exception table that maps instruction offsets to exception codes.
    exception_table: ExceptionTable,
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct ModuleStateMap {
    /// Local functions.
    pub local_functions: BTreeMap<usize, FunctionStateMap>,
    /// Total size.
    pub total_size: usize,
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct FunctionStateMap {
    /// Initial.
    pub initial: MachineState,
    /// Local Function Id.
    pub local_function_id: usize,
    /// Locals.
    pub locals: Vec<WasmAbstractValue>,
    /// Shadow size.
    pub shadow_size: usize, // for single-pass backend, 32 bytes on x86-64
    /// Diffs.
    pub diffs: Vec<MachineStateDiff>,
    /// Wasm Function Header target offset.
    pub wasm_function_header_target_offset: Option<SuspendOffset>,
    /// Wasm offset to target offset
    pub wasm_offset_to_target_offset: BTreeMap<usize, SuspendOffset>,
    /// Loop offsets.
    pub loop_offsets: BTreeMap<usize, OffsetInfo>, /* suspend_offset -> info */
    /// Call offsets.
    pub call_offsets: BTreeMap<usize, OffsetInfo>, /* suspend_offset -> info */
    /// Trappable offsets.
    pub trappable_offsets: BTreeMap<usize, OffsetInfo>, /* suspend_offset -> info */
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct MachineState {
    /// Stack values.
    pub stack_values: Vec<MachineValue>,
    /// Register values.
    pub register_values: Vec<MachineValue>,
    /// Previous frame.
    pub prev_frame: BTreeMap<usize, MachineValue>,
    /// Wasm stack.
    pub wasm_stack: Vec<WasmAbstractValue>,
    /// Private depth of the wasm stack.
    pub wasm_stack_private_depth: usize,
    /// Wasm instruction offset.
    pub wasm_inst_offset: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum MachineValue {
    /// Undefined.
    Undefined,
    /// Vmctx.
    Vmctx,
    /// Vmctx Deref.
    VmctxDeref(Vec<usize>),
    /// Preserve Register.
    PreserveRegister(RegisterIndex),
    /// Copy Stack BP Relative.
    CopyStackBPRelative(i32), // relative to Base Pointer, in byte offset
    /// Explicit Shadow.
    ExplicitShadow, // indicates that all values above this are above the shadow region
    /// Wasm Stack.
    WasmStack(usize),
    /// Wasm Local.
    WasmLocal(usize),
    /// Two Halves.
    TwoHalves(Box<(MachineValue, MachineValue)>), // 32-bit values. TODO: optimize: add another type for inner "half" value to avoid boxing?
}

impl BorshSerialize for MachineValue {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        match self {
            MachineValue::Undefined => writer.write_all(&[0u8])?,
            MachineValue::Vmctx => writer.write_all(&[1u8])?,
            MachineValue::VmctxDeref(v) => {
                writer.write(&[2u8])?;
                BorshSerialize::serialize(&v, writer)?;
            }
            MachineValue::PreserveRegister(r) => {
                writer.write(&[3u8])?;
                BorshSerialize::serialize(&r, writer)?;
            }
            MachineValue::CopyStackBPRelative(i) => {
                writer.write(&[4u8])?;
                BorshSerialize::serialize(&i, writer)?;
            }
            MachineValue::ExplicitShadow => writer.write_all(&(5 as u8).to_le_bytes())?,
            MachineValue::WasmStack(u) => {
                writer.write_all(&[6u8])?;
                BorshSerialize::serialize(&(*u as u64), writer)?;
            }
            MachineValue::WasmLocal(u) => {
                writer.write_all(&[7u8])?;
                BorshSerialize::serialize(&(*u as u64), writer)?;
            }
            MachineValue::TwoHalves(b) => {
                writer.write_all(&[8u8])?;
                BorshSerialize::serialize(&b, writer)?;
            }
        }
        Ok(())
    }
}

impl BorshDeserialize for MachineValue {
    fn deserialize(buf: &mut &[u8]) -> std::io::Result<Self> {
        let variant: u8 = BorshDeserialize::deserialize(buf)?;
        Ok(match variant {
            0 => MachineValue::Undefined,
            1 => MachineValue::Vmctx,
            2 => {
                let v: Vec<usize> = BorshDeserialize::deserialize(buf)?;
                MachineValue::VmctxDeref(v)
            }
            3 => {
                let r: RegisterIndex = BorshDeserialize::deserialize(buf)?;
                MachineValue::PreserveRegister(r)
            }
            4 => {
                let i: i32 = BorshDeserialize::deserialize(buf)?;
                MachineValue::CopyStackBPRelative(i)
            }
            5 => MachineValue::ExplicitShadow,
            6 => {
                let u: usize = BorshDeserialize::deserialize(buf)?;
                MachineValue::WasmStack(u)
            }
            7 => {
                let u: usize = BorshDeserialize::deserialize(buf)?;
                MachineValue::WasmLocal(u)
            }
            8 => {
                let b: Box<(MachineValue, MachineValue)> = BorshDeserialize::deserialize(buf)?;
                MachineValue::TwoHalves(b)
            }
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Unexpected variant",
                ))
            }
        })
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, BorshSerialize, BorshDeserialize)]
pub struct RegisterIndex(pub usize);

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, BorshSerialize, BorshDeserialize)]
pub enum WasmAbstractValue {
    /// A wasm runtime value
    Runtime,
    /// A wasm constant value
    Const(u64),
}

#[derive(Clone, Debug, Default, BorshSerialize, BorshDeserialize)]
pub struct MachineStateDiff {
    /// Last.
    pub last: Option<usize>,
    /// Stack push.
    pub stack_push: Vec<MachineValue>,
    /// Stack pop.
    pub stack_pop: usize,

    /// Register diff.
    pub reg_diff: Vec<(RegisterIndex, MachineValue)>,

    /// Previous frame diff.
    pub prev_frame_diff: BTreeMap<usize, Option<MachineValue>>, // None for removal

    /// Wasm stack push.
    pub wasm_stack_push: Vec<WasmAbstractValue>,
    /// Wasm stack pop.
    pub wasm_stack_pop: usize,
    /// Private depth of the wasm stack.
    pub wasm_stack_private_depth: usize, // absolute value; not a diff.
    /// Wasm instruction offset.
    pub wasm_inst_offset: usize, // absolute value; not a diff.
}

#[derive(Clone, Copy, Debug, BorshSerialize, BorshDeserialize)]
pub enum SuspendOffset {
    /// A loop.
    Loop(usize),
    /// A call.
    Call(usize),
    /// A trappable.
    Trappable(usize),
}

#[derive(Clone, Debug, Default, BorshDeserialize)]
pub struct ExceptionTable {
    /// Mappings from offsets in generated machine code to the corresponding exception code.
    pub offset_to_code: HashMap<usize, ExceptionCode>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, BorshSerialize, BorshDeserialize)]
pub enum ExceptionCode {
    /// An `unreachable` opcode was executed.
    Unreachable = 0,
    /// Call indirect incorrect signature trap.
    IncorrectCallIndirectSignature = 1,
    /// Memory out of bounds trap.
    MemoryOutOfBounds = 2,
    /// Call indirect out of bounds trap.
    CallIndirectOOB = 3,
    /// An arithmetic exception, e.g. divided by zero.
    IllegalArithmetic = 4,
    /// Misaligned atomic access trap.
    MisalignedAtomicAccess = 5,
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct OffsetInfo {
    /// End offset.
    pub end_offset: usize, // excluded bound
    /// Diff Id.
    pub diff_id: usize,
    /// Activate offset.
    pub activate_offset: usize,
}

impl BorshDeserialize for CacheImage {
    fn deserialize(buf: &mut &[u8]) -> std::io::Result<Self> {
        let t1 = std::time::Instant::now();
        let code: Vec<u8> = BorshDeserialize::deserialize(buf)?;
        let t2 = std::time::Instant::now();
        let function_pointers = BorshDeserialize::deserialize(buf)?;
        let t3 = std::time::Instant::now();
        let function_offsets = BorshDeserialize::deserialize(buf)?;
        let t4 = std::time::Instant::now();
        let func_import_count: u64 = BorshDeserialize::deserialize(buf)?;
        let t5 = std::time::Instant::now();
        let func_import_count = func_import_count as usize;
        let t6 = std::time::Instant::now();
        let msm: ModuleStateMap = BorshDeserialize::deserialize(buf)?;
        let t7 = std::time::Instant::now();
        let exception_table: ExceptionTable = BorshDeserialize::deserialize(buf)?;
        let t8 = std::time::Instant::now();
        println!(
            "{:?} {:?} {:?} {:?} {:?} {:?} {:?}",
            t2 - t1,
            t3 - t2,
            t4 - t3,
            t5 - t4,
            t6 - t5,
            t7 - t6,
            t8 - t7
        );
        Ok(Self {
            code,
            function_pointers,
            function_offsets,
            func_import_count,
            msm,
            exception_table,
        })
    }
}

fn main() {
    println!("Hello, world!");
    let mut buffer = Vec::<u8>::new();
    {
        use std::io::Read;
        let mut file = std::fs::File::open("cache_image").unwrap();
        // read the same file back into a Vec of bytes
        file.read_to_end(&mut buffer).unwrap();
        // println!("{:?}", buffer);
    }
    let t1 = std::time::Instant::now();
    let cache_image: CacheImage = BorshDeserialize::deserialize(&mut buffer.as_ref()).unwrap();
    let t2 = std::time::Instant::now();
    println!("{:?} {:?}", t2 - t1, cache_image.code.len());
}

use std::collections::hash_map::RandomState;
pub struct HashMap2<K, V, S = RandomState> {
    pub base: HashMap3<K, V, S>,
}

pub struct HashMap3<K, V, S> {
    pub hash_builder: S,
    pub table: RawTable<(K, V)>,
}

use core::ptr::NonNull;
use core::marker::PhantomData;
pub struct RawTable<T> {
    pub bucket_mask: usize,
    pub ctrl: NonNull<u8>,
    pub growth_left: usize,
    pub items: usize,
    pub marker: PhantomData<T>,
}

use std::mem;
impl BorshSerialize for ExceptionTable {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        let mut std_hashmap: HashMap2<usize, ExceptionCode> = unsafe {
            mem::transmute_copy(&self.offset_to_code)
        };
        let mut hashbrown_raw_table = std_hashmap.base.table;
        BorshSerialize::serialize(&hashbrown_raw_table.bucket_mask, writer)?;
        BorshSerialize::serialize(&hashbrown_raw_table.growth_left, writer)?;
        BorshSerialize::serialize(&hashbrown_raw_table.items, writer)?;
        let buckets = hashbrown_raw_table.bucket_mask+1;
        let mut data_start = unsafe{NonNull::new_unchecked(hashbrown_raw_table.ctrl.as_ptr() as *mut (usize, ExceptionCode)).as_ptr().wrapping_sub(buckets)};
        BorshSerialize::serialize(&unsafe{*std::ptr::slice_from_raw_parts(data_start, buckets+buckets+16)}, writer)

    }
}


