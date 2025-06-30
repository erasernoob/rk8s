use crate::{
    commands::{create, delete, exec, exec::Exec, exec_cli::ExecContainer, load_container, start},
    cri::cri_api::{
        ContainerConfig, ContainerMetadata, CreateContainerResponse, ImageSpec, KeyValue, Mount,
        StartContainerResponse,
    },
    rootpath,
    task::{ContainerSpec, add_cap_net_raw, get_linux_container_config},
};
use anyhow::{Ok, Result, anyhow};
use liboci_cli::{Create, Delete, Start};
use oci_spec::runtime::{
    LinuxBuilder, LinuxNamespaceBuilder, LinuxNamespaceType, ProcessBuilder, Spec,
};
use std::{
    fs::File,
    io::{BufWriter, Read, Write},
    path::Path,
};

pub struct ContainerRunner {
    sepc: ContainerSpec,
    config: Option<ContainerConfig>,
    id: String,
}

impl ContainerRunner {
    pub fn from_spec(spec: ContainerSpec) -> Result<Self> {
        let id = spec.name.clone();
        Ok(ContainerRunner {
            sepc: spec,
            config: None,
            id: id,
        })
    }

    pub fn from_file(spec_path: &str) -> Result<Self> {
        // read the container_spec bytes
        let mut file = File::open(spec_path)
            .map_err(|e| anyhow!("open the container spec file failed: {e}"))?;
        let mut content = String::new();
        file.read_to_string(&mut content)?;

        let container_spec: ContainerSpec = serde_yaml::from_str(&content)?;
        let container_id = container_spec.name.clone();
        Ok(ContainerRunner {
            sepc: (container_spec),
            config: None,
            id: container_id,
        })
    }

    pub fn from_container_id(id: &str) -> Result<Self> {
        Ok(ContainerRunner {
            sepc: ContainerSpec {
                name: id.to_string(),
                image: "".to_string(),
                ports: vec![],
                args: vec![],
                resources: None,
            },
            config: None,
            id: id.to_string(),
        })
    }

    pub fn run(&mut self) -> Result<()> {
        let _ = self.create_container()?;
        // create container_config
        self.build_config()?;

        let id = self.id.as_str();
        // See if the container exists
        match is_container_exist(id).is_ok() {
            // exist
            true => {
                self.start_container(None)?;
                println!("Container: {id} runs successfully!");
                Ok(())
            }
            // not exist
            false => {
                // create container
                let CreateContainerResponse { container_id } = self.create_container()?;
                self.start_container(None)?;
                println!("Container: {container_id} runs successfully!");
                Ok(())
            }
        }
    }

    pub fn get_container_state(&self) -> Result<String> {
        let root_path = rootpath::determine(None)?;
        let container = load_container(root_path, &self.id)?;
        Ok(serde_json::to_string_pretty(&container.state)?)
    }

    pub fn get_container_id(&self) -> Result<String> {
        let container_id = self
            .config
            .as_ref()
            .and_then(|c| c.metadata.as_ref().map(|m| m.name.clone()))
            .ok_or_else(|| anyhow!("Failed to get the container id"))?;
        Ok(container_id)
    }

    pub fn build_config(&mut self) -> Result<()> {
        let config = ContainerConfig {
            metadata: Some(ContainerMetadata {
                name: self.sepc.name.clone(),
                attempt: 0,
            }),
            image: Some(ImageSpec {
                image: self.sepc.image.clone(),
                annotations: std::collections::HashMap::new(),
                user_specified_image: self.sepc.image.clone(),
                runtime_handler: String::new(),
            }),
            command: vec!["bin/sh".to_string()],
            args: self.sepc.args.clone(),
            working_dir: String::from("/"),
            envs: vec![KeyValue {
                key: "PATH".to_string(),
                value: "usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
            }],
            mounts: vec![
                Mount {
                    container_path: "/proc".to_string(),
                    host_path: "proc".to_string(),
                    readonly: false,
                    selinux_relabel: false,
                    propagation: 0,
                    uid_mappings: vec![],
                    gid_mappings: vec![],
                    recursive_read_only: false,
                    image: None,
                    image_sub_path: "".to_string(),
                },
                Mount {
                    container_path: "/dev".to_string(),
                    host_path: "tmpfs".to_string(),
                    readonly: false,
                    selinux_relabel: false,
                    propagation: 0,
                    uid_mappings: vec![],
                    gid_mappings: vec![],
                    recursive_read_only: false,
                    image: None,
                    image_sub_path: "".to_string(),
                },
            ],
            devices: vec![],
            labels: std::collections::HashMap::new(),
            annotations: std::collections::HashMap::new(),
            log_path: format!("{}/0.log", self.sepc.name),
            stdin: false,
            stdin_once: false,
            tty: false,
            linux: get_linux_container_config(self.sepc.resources.clone())?,
            windows: None,
            cdi_devices: vec![],
            stop_signal: 0,
        };
        self.config = std::option::Option::Some(config);

        Ok(())
    }

    pub fn create_container(&self) -> Result<CreateContainerResponse> {
        // validate the config
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| anyhow!("Container's Config is required"))?;

        let container_id = self.get_container_id()?;

        //  create oci spec
        let mut spec = Spec::default();

        // use the default namesapce configuration
        let namespaces = vec![
            LinuxNamespaceBuilder::default()
                .typ(LinuxNamespaceType::Pid)
                .build()?,
            LinuxNamespaceBuilder::default()
                .typ(LinuxNamespaceType::Network)
                .build()?,
            LinuxNamespaceBuilder::default()
                .typ(LinuxNamespaceType::Ipc)
                .build()?,
            LinuxNamespaceBuilder::default()
                .typ(LinuxNamespaceType::Uts)
                .build()?,
            LinuxNamespaceBuilder::default()
                .typ(LinuxNamespaceType::Mount)
                .build()?,
            LinuxNamespaceBuilder::default()
                .typ(LinuxNamespaceType::Cgroup)
                .build()?,
        ];

        let mut linux: LinuxBuilder = LinuxBuilder::default().namespaces(namespaces);
        if let Some(x) = &config.linux {
            if let Some(r) = &x.resources {
                linux = linux.resources(r);
            }
        }
        let linux = linux.build()?;
        spec.set_linux(Some(linux));

        // build the procss path
        let mut process = ProcessBuilder::default()
            .args(self.sepc.args.clone())
            .build()?;

        let mut capabilities = process.capabilities().clone().unwrap();
        // add the CAP_NET_RAW
        add_cap_net_raw(&mut capabilities);
        process.set_capabilities(Some(capabilities));
        spec.set_process(Some(process));

        // create a config.path at the bundle path
        let bundle_path = self.sepc.image.clone();
        if bundle_path.is_empty() {
            return Err(anyhow!("Bundle path (image) is empty"));
        }
        let bundle_dir = Path::new(&bundle_path);
        if !bundle_dir.exists() {
            return Err(anyhow!("Bundle directory does not exist"));
        }

        let config_path = format!("{}/config.json", bundle_path);
        if Path::new(&config_path).exists() {
            std::fs::remove_file(&config_path).map_err(|e| {
                anyhow!(
                    "Failed to remove existing config.json in bundle path: {}",
                    e
                )
            })?;
        }
        let file = File::create(config_path)?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, &spec)?;
        writer.flush()?;

        let create_args = Create {
            bundle: bundle_path.clone().into(),
            console_socket: None,
            pid_file: None,
            no_pivot: false,
            no_new_keyring: false,
            preserve_fds: 0,
            container_id: container_id.clone(),
        };

        let root_path = rootpath::determine(None)
            .map_err(|e| anyhow!("Failed to determine the rootpath: {}", e))?;

        create::create(create_args, root_path, false)
            .map_err(|e| anyhow!("Failed to create container: {}", e))?;

        return Ok(CreateContainerResponse {
            container_id: container_id,
        });
    }

    pub fn start_container(&self, id: Option<String>) -> Result<StartContainerResponse> {
        let root_path = rootpath::determine(None)?;
        match id {
            None => {
                let id = self.get_container_id()?;
                start::start(
                    Start {
                        container_id: id.clone(),
                    },
                    root_path,
                )?;

                Ok(StartContainerResponse {})
            }
            Some(id) => {
                start::start(
                    Start {
                        container_id: id.clone(),
                    },
                    root_path,
                )?;

                Ok(StartContainerResponse {})
            }
        }
    }
}

pub fn run_container(path: &str) -> Result<(), anyhow::Error> {
    // read the container_spec bytes to container_spec struct
    let mut runner = ContainerRunner::from_file(path)?;

    // create container_config
    runner.build_config()?;

    let id = runner.id.as_str();
    // See if the container exists
    match is_container_exist(id).is_ok() {
        // exist
        true => {
            runner.start_container(None)?;
            println!("Container: {id} runs successfully!");
            Ok(())
        }
        // not exist
        false => {
            // create container
            let CreateContainerResponse { container_id } = runner.create_container()?;
            runner.start_container(None)?;
            println!("Container: {container_id} runs successfully!");
            Ok(())
        }
    }
}

pub fn is_container_exist(id: &str) -> Result<()> {
    let root_path = rootpath::determine(None)?;
    let _ = load_container(root_path, id)?;
    Ok(())
}

pub fn state_container(id: &str) -> Result<()> {
    let root_path = rootpath::determine(None)?;
    println!("ROOT PATH: {}", root_path.to_str().unwrap_or_default());

    let container = load_container(root_path, id)?;
    println!("{}", serde_json::to_string_pretty(&container.state)?);

    Ok(())
}

pub fn delete_container(id: &str) -> Result<()> {
    is_container_exist(id)?;
    let root_path = rootpath::determine(None)?;
    let delete_args = Delete {
        container_id: id.to_string(),
        force: true,
    };
    let _ = delete::delete(delete_args, root_path)?;

    Ok(())
}

pub fn start_container(container_id: &str) -> Result<()> {
    let runner = ContainerRunner::from_container_id(container_id)?;
    runner.start_container(Some(container_id.to_string()))?;
    println!("container {container_id} start successfully");
    Ok(())
}
pub fn exec_container(args: ExecContainer) -> Result<i32> {
    let rootpath = rootpath::determine(None)?;
    let args = Exec::from(args);

    let exit_code = exec::exec(args, rootpath)?;
    Ok(exit_code)
}

pub fn create_container(path: &str) -> Result<()> {
    let mut runner = ContainerRunner::from_file(path)?;
    runner.run()
}

#[cfg(test)]
mod test {
    use core::panic;

    use serial_test::serial;

    use super::*;
    // fn runner_from_file(path: &str) -> Result<ContainerRunner> {
    //     let mut runner = ContainerRunner::from_file(path)?;
    //     runner.build_config()?;

    //     Ok(runner)
    // }

    #[test]
    #[serial]
    fn test_run_container() {
        let path = "/home/erasernoob/project/rk8s/project/test/single_container.yaml";
        println!("{path}");
        let runner = ContainerRunner::from_container_id(path)
            .unwrap_or_else(|e| panic!("Initialize the Runner failed:{e} "));
        runner
            .create_container()
            .unwrap_or_else(|e| panic!("Got Runner failed: {e}"));
    }

    #[test]
    #[serial]
    fn test_start_container() {
        let container_id = "main-container1";
        let runner = ContainerRunner::from_container_id(container_id)
            .unwrap_or_else(|e| panic!("Initialize the Runner failed:{e} "));
        let _ = runner
            .start_container(Some(container_id.to_string()))
            .unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    #[serial]
    fn test_the_state() {}
}
