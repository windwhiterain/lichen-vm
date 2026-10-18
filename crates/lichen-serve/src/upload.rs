//! Making a resident out of host data, which is the one thing a service needs and nothing else does.

use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_kernel_ir::ScalarClass;

use crate::{ServeRefusal, Session};

/// A step whose only job is to copy a host buffer into a resident.
///
/// # Invariant
/// Two dispatches, because a one-node step cannot hand an answer back. The copy's body is the
/// identity, so the second dispatch writes what the first one wrote: what changes is where the data
/// lives, which is the whole point of an upload.
const UPLOAD: &str = r#"
--- compute = import "compute.lichen" ---
In = struct{.d (compute.Buf Float)}
Out = struct{.e (compute.Buf Float)}
Par = struct{.n Int, .in In, .out Out}
copy_raw = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value ((compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float))))
}
copy = compute.parallel copy_raw "gpu" Out
GArg = struct{.n Int, .in In}
step = (s : GArg) => {
  a = (compute.plrun copy ((compute.A In)(.n s.n, .I In(.d s.in.d))) : Out)
  compute.plrun copy ((compute.A In)(.n s.n, .I In(.d a.e)))
}
built = compute.graph step
built
"#;

impl Session {
    /// Make a resident out of host data, which is what a model's weights need to stay on the device.
    ///
    /// # Invariant
    /// A step's output is the only other way to get a resident, so this prepares and runs the
    /// copying step on every call: the compile is paid once per upload rather than once per token,
    /// which is what makes it the right shape for a weight load. The resident it answers is the
    /// caller's to release.
    pub fn upload(&self, values: &[f32]) -> Result<ResidentBuffer, ServeRefusal> {
        let prepared = self.prepare(UPLOAD)?;
        let stepped = prepared.step(vec![
            RunArgument::Count(values.len() as i64),
            RunArgument::Buffer {
                class: ScalarClass::Float,
                data: values
                    .iter()
                    .flat_map(|value| value.to_le_bytes())
                    .collect(),
            },
        ])?;
        match &stepped.results[..] {
            [RunResult::Resident(resident)] => Ok(*resident),
            _ => Err(ServeRefusal::Program(vec![
                "the copying step answers with one resident, and this one answered otherwise"
                    .into(),
            ])),
        }
    }
}

impl Session {
    /// Make a resident out of the checkpoint's own halves, which is the width a model keeps them at.
    ///
    /// # Invariant
    /// The copying step is the one above with a pack stating `BFloat16`, so what crosses to the device is
    /// two bytes an element and the resident is classed as it was written: a half-width model that held
    /// f32 residents would pay four bytes a weight it never reads at that width.
    pub fn upload_narrowed(&self, halves: &[u8]) -> Result<ResidentBuffer, ServeRefusal> {
        let prepared = crate::narrowed(self, UPLOAD).map_err(|refusal| {
            ServeRefusal::Program(vec![format!("the copying step narrows: {refusal}")])
        })?;
        let stepped = prepared.step(vec![
            RunArgument::Count((halves.len() / 2) as i64),
            RunArgument::Buffer {
                class: ScalarClass::BFloat16,
                data: halves.to_vec(),
            },
        ])?;
        match &stepped.results[..] {
            [RunResult::Resident(resident)] => Ok(*resident),
            _ => Err(ServeRefusal::Program(vec![
                "the copying step answers with one resident, and this one answered otherwise"
                    .into(),
            ])),
        }
    }
}
