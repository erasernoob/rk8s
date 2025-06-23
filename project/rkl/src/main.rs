use clap::{Parser, Subcommand, ValueEnum};
use rkl::cli_commands;

#[derive(Parser)]
#[command(name = "rkl")]
#[command(about = "A simple container runtime", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Clone, ValueEnum)]
enum RunType {
    #[clap(name = "pod")]
    Pod,
    #[clap(name = "container")]
    Container,
    #[clap(name = "compose")]
    Compose,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Run the container/pod.yaml/compose.yml")]
    Run {
        #[arg(value_name = "RUN_TYPE")]
        run_type: RunType,
    },
    #[command(about = "Create a pod from a YAML file using ./rkl create pod.yaml")]
    Create {
        #[arg(value_name = "POD_YAML")]
        pod_yaml: String,
    },
    #[command(about = "Start a pod with a pod-name using ./rkl start pod-name")]
    Start {
        #[arg(value_name = "POD_NAME")]
        pod_name: String,
    },

    #[command(about = "Delete a pod with a pod-name using ./rkl delete pod-name")]
    Delete {
        #[arg(value_name = "POD_NAME")]
        pod_name: String,
    },
    #[command(about = "Get the state of a pod using ./rkl state pod-name")]
    State {
        #[arg(value_name = "POD_NAME")]
        pod_name: String,
    },
    Exec(Box<rkl::commands::exec_cli::Exec>),
}

fn main() -> Result<(), anyhow::Error> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run { pod_yaml } => cli_commands::run_pod(&pod_yaml),
        Commands::Create { pod_yaml } => cli_commands::create_pod(&pod_yaml),
        Commands::Start { pod_name } => cli_commands::start_pod(&pod_name),
        Commands::Delete { pod_name } => cli_commands::delete_pod(&pod_name),
        Commands::State { pod_name } => cli_commands::state_pod(&pod_name),
        Commands::Exec(exec) => {
            let exit_code = cli_commands::exec_pod(*exec)?;
            std::process::exit(exit_code);
        }
    }
}
