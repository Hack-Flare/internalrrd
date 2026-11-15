use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::BytesMut;
use futures_util::{SinkExt, StreamExt};
use protocol::{
    Capability, CodecError, Command, DeploymentRejected, HelloRequest, HelloResponse,
    InstanceStatus, MetricsRequest, Outcome, ProtocolError, ProtocolErrorCode, ProtocolVersion,
    RejectionCode, Request, RequestCodec, Response, ResponseCodec, SignedDeployment, StatusReport,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_util::codec::{Decoder, Encoder, Framed};

use crate::deployment::Deployment;
use crate::metrics;
use crate::supervisor::Supervisor;

/// Shared state used by management connections.
#[derive(Clone)]
pub struct ManagementContext {
    application: Arc<ApplicationInfo>,
    supervisor: Arc<Mutex<Supervisor>>,
    deployment: Option<Arc<Deployment>>,
}

struct ApplicationInfo {
    name: String,
    working_directory: Option<PathBuf>,
}

impl ManagementContext {
    pub fn new(
        application_name: impl Into<String>,
        working_directory: Option<PathBuf>,
        supervisor: Arc<Mutex<Supervisor>>,
    ) -> Self {
        Self {
            application: Arc::new(ApplicationInfo {
                name: application_name.into(),
                working_directory,
            }),
            supervisor,
            deployment: None,
        }
    }

    pub fn with_deployment(mut self, deployment: Arc<Deployment>) -> Self {
        self.deployment = Some(deployment);
        self
    }

    fn application_name(&self) -> &str {
        &self.application.name
    }

    fn configured_working_directory(&self) -> Option<&Path> {
        self.application.working_directory.as_deref()
    }

    async fn current_version(&self) -> Option<String> {
        match &self.deployment {
            Some(deployment) => deployment.current_version().await,
            None => None,
        }
    }

    async fn active_working_directory(&self) -> Option<PathBuf> {
        if let Some(deployment) = &self.deployment {
            if let Some(release) = deployment.current_release().await {
                return Some(release);
            }
            if let Some(directory) = deployment.configured_working_directory() {
                return Some(directory.to_path_buf());
            }
        }
        self.configured_working_directory().map(Path::to_path_buf)
    }
}

/// Accept management connections until the listener fails.
pub async fn serve(listener: TcpListener, context: ManagementContext) -> io::Result<()> {
    loop {
        let (stream, peer) = listener.accept().await?;
        let context = context.clone();

        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, context).await {
                tracing::warn!(%peer, error = %error, "management connection failed");
            }
        });
    }
}

async fn handle_connection(
    stream: TcpStream,
    context: ManagementContext,
) -> Result<(), ConnectionError> {
    let peer = stream.peer_addr()?;
    tracing::debug!(%peer, "management connection opened");
    let mut framed = Framed::new(stream, ServerCodec::default());

    while let Some(request) = framed.next().await {
        let request = request?;
        let response = handle_request(request, &context).await;
        framed.send(response).await?;
    }

    tracing::debug!(%peer, "management connection closed");
    Ok(())
}

async fn handle_request(request: Request, context: &ManagementContext) -> Response {
    let version = request.version;
    let request_id = request.request_id;

    let outcome = if version.is_compatible(ProtocolVersion::CURRENT) {
        match request.body {
            Command::Hello(hello) => handle_hello(hello, context).await,
            Command::Status => handle_status(context).await,
            Command::Metrics(metrics_request) => handle_metrics(metrics_request, context).await,
            Command::Deploy(deployment) => handle_deploy(deployment, context).await,
        }
    } else {
        Outcome::Error(ProtocolError {
            code: ProtocolErrorCode::UNSUPPORTED_VERSION,
            message: format!("unsupported protocol version {version}"),
        })
    };

    Response::new(version, request_id, outcome)
}

async fn handle_hello(_request: HelloRequest, context: &ManagementContext) -> Outcome {
    Outcome::Hello(HelloResponse {
        application: context.application_name().to_string(),
        capabilities: vec![
            Capability::SIGNED_DEPLOYMENTS,
            Capability::STATUS_REPORTING,
            Capability::METRICS_REPORTING,
        ],
        deployed_version: context.current_version().await,
    })
}

async fn handle_status(context: &ManagementContext) -> Outcome {
    let instances = match refresh_instances(context).await {
        Ok(instances) => instances,
        Err(error) => return protocol_error(error),
    };
    let deployed_version = context.current_version().await;

    Outcome::Status(StatusReport {
        application: context.application_name().to_string(),
        deployed_version,
        instances: instances
            .into_iter()
            .map(|instance| InstanceStatus {
                id: instance.id.get(),
                pid: instance.pid,
                healthy: true,
                uptime_secs: instance.uptime.as_secs(),
            })
            .collect(),
    })
}

async fn handle_metrics(request: MetricsRequest, context: &ManagementContext) -> Outcome {
    let instances = match refresh_instances(context).await {
        Ok(instances) => instances,
        Err(error) => return protocol_error(error),
    };

    let application_name = context.application_name().to_string();
    let working_directory = context.active_working_directory().await;
    let deployed_version = context.current_version().await;
    let instance = request.instance;
    let result = tokio::task::spawn_blocking(move || {
        metrics::collect(
            &application_name,
            deployed_version,
            working_directory.as_deref(),
            &instances,
            instance,
        )
    })
    .await;

    match result {
        Ok(report) => Outcome::Metrics(Box::new(report)),
        Err(error) => protocol_error(format!("metrics collection failed: {error}")),
    }
}

async fn handle_deploy(signed: SignedDeployment, context: &ManagementContext) -> Outcome {
    tracing::info!(
        application = %signed.deployment.application,
        version = %signed.deployment.version,
        "received a deployment"
    );

    match &context.deployment {
        Some(deployment) => deployment.deploy(signed).await,
        None => Outcome::DeploymentRejected(DeploymentRejected {
            code: RejectionCode::INTERNAL,
            message: "deployment handling is not configured".to_string(),
        }),
    }
}

async fn refresh_instances(
    context: &ManagementContext,
) -> Result<Vec<crate::supervisor::InstanceInfo>, String> {
    let mut supervisor = context.supervisor.lock().await;
    supervisor.poll().map_err(|error| error.to_string())?;
    Ok(supervisor.running())
}

fn protocol_error(message: String) -> Outcome {
    Outcome::Error(ProtocolError {
        code: ProtocolErrorCode::INTERNAL,
        message,
    })
}

#[derive(Debug, thiserror::Error)]
enum ConnectionError {
    #[error("management connection I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("management protocol error: {0}")]
    Codec(#[from] CodecError),
}

#[derive(Default)]
struct ServerCodec {
    requests: RequestCodec,
    responses: ResponseCodec,
}

impl Decoder for ServerCodec {
    type Item = Request;
    type Error = CodecError;

    fn decode(&mut self, source: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.requests.decode(source)
    }
}

impl Encoder<Response> for ServerCodec {
    type Error = CodecError;

    fn encode(&mut self, item: Response, destination: &mut BytesMut) -> Result<(), Self::Error> {
        self.responses.encode(item, destination)
    }
}
