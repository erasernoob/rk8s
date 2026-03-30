use crate::sandbox::protocol::GuestReadyEvent;
use anyhow::{Context, Result};
use async_trait::async_trait;
use clap::Args;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::path::PathBuf;

pub mod firecracker;
pub mod libkrun;

pub use firecracker::FirecrackerVmBackend;
pub use libkrun::LibkrunVmBackend;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmInstanceSpec {
    pub sandbox_id: String,
    pub image: String,
    pub cpus: u32,
    pub memory_mib: u32,
    pub persistent: bool,
    pub kernel_path: Option<PathBuf>,
    pub initrd_path: Option<PathBuf>,
    pub guest_image_path: Option<PathBuf>,
    pub work_dir: PathBuf,
    pub ready_file: PathBuf,
    pub parent_pid: Option<u32>,
    pub vmm_kind: VmmKind,
    pub boot_args: Option<String>,
    pub firecracker_api_socket: PathBuf,
    pub vsock_uds_path: PathBuf,
    pub agent_socket_path: PathBuf,
    pub ready_socket_path: PathBuf,
    pub guest_cid: u32,
    pub agent_vsock_port: u32,
    pub ready_vsock_port: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmInstanceHandle {
    pub sandbox_id: String,
    pub vm_id: String,
    pub pid: Option<u32>,
    pub shim_pid: Option<u32>,
    pub control_socket: Option<PathBuf>,
    pub ready_file: PathBuf,
    pub work_dir: PathBuf,
    pub vmm_kind: VmmKind,
    pub vsock_uds_path: Option<PathBuf>,
    pub agent_socket_path: Option<PathBuf>,
    pub ready_socket_path: Option<PathBuf>,
}

#[async_trait]
pub trait VmBackend: Send + Sync {
    async fn boot(&self, spec: &VmInstanceSpec) -> Result<VmInstanceHandle>;
    async fn wait_ready(&self, handle: &VmInstanceHandle) -> Result<GuestReadyEvent>;
    async fn stop(&self, handle: &VmInstanceHandle) -> Result<()>;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum VmmKind {
    Firecracker,
    Libkrun,
}

impl VmmKind {
    pub const DEFAULT: Self = Self::Libkrun;

    pub fn from_env() -> Result<Self> {
        match std::env::var("RKFORGE_SANDBOX_VMM") {
            Ok(value) => Self::parse(&value),
            Err(std::env::VarError::NotPresent) => Ok(Self::DEFAULT),
            Err(err) => Err(err).context("failed to read RKFORGE_SANDBOX_VMM"),
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "firecracker" | "fc" => Ok(Self::Firecracker),
            "libkrun" | "krun" => Ok(Self::Libkrun),
            other => anyhow::bail!(
                "unsupported sandbox VMM kind `{other}`; expected `firecracker` or `libkrun`"
            ),
        }
    }
}

#[derive(Debug, Args)]
pub struct SandboxShimArgs {
    #[arg(long)]
    pub spec: PathBuf,
}

const DEFAULT_GUEST_CID: u32 = 3;
const DEFAULT_AGENT_VSOCK_PORT: u32 = 26_950;
const DEFAULT_READY_VSOCK_PORT: u32 = 26_951;

pub fn build_vm_spec(
    root: &Path,
    sandbox_id: &str,
    image: &str,
    cpus: u32,
    memory_mib: u32,
    persistent: bool,
    vmm_kind: VmmKind,
) -> VmInstanceSpec {
    let work_dir = root.join("instances").join(sandbox_id);
    let guest_image_path = std::env::var_os("RKFORGE_SANDBOX_GUEST_IMAGE").map(PathBuf::from);
    let kernel_path = std::env::var_os("RKFORGE_SANDBOX_KERNEL").map(PathBuf::from);
    let initrd_path = std::env::var_os("RKFORGE_SANDBOX_INITRD").map(PathBuf::from);
    VmInstanceSpec {
        sandbox_id: sandbox_id.to_string(),
        image: image.to_string(),
        cpus,
        memory_mib,
        persistent,
        kernel_path,
        initrd_path,
        guest_image_path,
        ready_file: work_dir.join("guest-ready.json"),
        work_dir: work_dir.clone(),
        parent_pid: Some(std::process::id()),
        vmm_kind,
        boot_args: None,
        firecracker_api_socket: work_dir.join("firecracker.socket"),
        vsock_uds_path: work_dir.join("guest.vsock"),
        agent_socket_path: work_dir.join("agent.sock"),
        ready_socket_path: work_dir.join("ready.sock"),
        guest_cid: DEFAULT_GUEST_CID,
        agent_vsock_port: DEFAULT_AGENT_VSOCK_PORT,
        ready_vsock_port: DEFAULT_READY_VSOCK_PORT,
    }
}

pub fn run_shim_command(args: SandboxShimArgs) -> Result<()> {
    let bytes = fs::read(&args.spec)
        .with_context(|| format!("failed to read shim spec {}", args.spec.display()))?;
    let spec: VmInstanceSpec =
        serde_json::from_slice(&bytes).with_context(|| "failed to parse shim spec")?;

    match spec.vmm_kind {
        VmmKind::Firecracker => firecracker::run_firecracker_shim(spec),
        VmmKind::Libkrun => libkrun::run_libkrun_shim(spec),
    }
}
