use std::net::SocketAddr;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use protocol::{
    CodecError, Command, DeploymentAccepted, HelloRequest, HelloResponse, MetricsReport, Outcome,
    Request, RequestCodec, RequestId, Response, ResponseCodec, SignedDeployment, StatusReport,
};
use tokio::net::TcpStream;
use tokio_util::codec::{Decoder, Encoder, Framed};

/// A control plane connection to one management server.
pub struct Client {
    framed: Framed<TcpStream, ClientCodec>,
    timeout: Duration,
    next_request_id: u64,
    hello: HelloResponse,
}

impl Client {
    /// Connect to a server and complete the hello handshake.
    pub async fn connect(
        address: SocketAddr,
        application: &str,
        timeout: Duration,
    ) -> Result<Self, ClientError> {
        let stream = tokio::time::timeout(timeout, TcpStream::connect(address))
            .await
            .map_err(|_| ClientError::Timeout(timeout))?
            .map_err(|source| ClientError::Connect { address, source })?;
        let mut framed = Framed::new(stream, ClientCodec::default());

        let outcome = send_request(
            &mut framed,
            timeout,
            0,
            Command::Hello(HelloRequest {
                capabilities: Vec::new(),
            }),
        )
        .await?;
        let hello = match outcome {
            Outcome::Hello(hello) => hello,
            _ => {
                return Err(ClientError::Unexpected { request: "hello" });
            }
        };

        if hello.application != application {
            return Err(ClientError::ApplicationMismatch {
                expected: application.to_string(),
                actual: hello.application.clone(),
            });
        }

        Ok(Self {
            framed,
            timeout,
            next_request_id: 1,
            hello,
        })
    }

    /// The hello response returned by the server.
    pub fn hello(&self) -> &HelloResponse {
        &self.hello
    }

    /// Send a signed deployment and wait for acceptance.
    pub async fn deploy(
        &mut self,
        signed: SignedDeployment,
    ) -> Result<DeploymentAccepted, ClientError> {
        match self.request(Command::Deploy(signed)).await? {
            Outcome::DeploymentAccepted(accepted) => Ok(accepted),
            Outcome::DeploymentRejected(rejected) => Err(ClientError::Rejected {
                code: rejected.code.raw(),
                message: rejected.message,
            }),
            _ => Err(ClientError::Unexpected { request: "deploy" }),
        }
    }

    /// Request the server status.
    pub async fn status(&mut self) -> Result<StatusReport, ClientError> {
        match self.request(Command::Status).await? {
            Outcome::Status(status) => Ok(status),
            _ => Err(ClientError::Unexpected { request: "status" }),
        }
    }

    /// Request a metrics snapshot.
    pub async fn metrics(&mut self, instance: Option<u64>) -> Result<MetricsReport, ClientError> {
        match self
            .request(Command::Metrics(protocol::MetricsRequest { instance }))
            .await?
        {
            Outcome::Metrics(report) => Ok(*report),
            _ => Err(ClientError::Unexpected { request: "metrics" }),
        }
    }

    async fn request(&mut self, command: Command) -> Result<Outcome, ClientError> {
        let request_id = self.next_request_id;
        self.next_request_id = request_id
            .checked_add(1)
            .ok_or(ClientError::RequestIdsExhausted)?;

        send_request(&mut self.framed, self.timeout, request_id, command).await
    }
}

async fn send_request(
    framed: &mut Framed<TcpStream, ClientCodec>,
    timeout: Duration,
    request_id: u64,
    command: Command,
) -> Result<Outcome, ClientError> {
    let request = Request::current(RequestId::new(request_id), command);

    tokio::time::timeout(timeout, framed.send(request))
        .await
        .map_err(|_| ClientError::Timeout(timeout))?
        .map_err(ClientError::Send)?;

    let response = tokio::time::timeout(timeout, framed.next())
        .await
        .map_err(|_| ClientError::Timeout(timeout))?
        .ok_or(ClientError::Closed)?
        .map_err(ClientError::Receive)?;

    if response.request_id.get() != request_id {
        return Err(ClientError::RequestIdMismatch {
            expected: request_id,
            actual: response.request_id.get(),
        });
    }

    match response.body {
        Outcome::Error(error) => Err(ClientError::Protocol {
            code: error.code.raw(),
            message: error.message,
        }),
        outcome => Ok(outcome),
    }
}

#[derive(Default)]
struct ClientCodec {
    requests: RequestCodec,
    responses: ResponseCodec,
}

impl Encoder<Request> for ClientCodec {
    type Error = CodecError;

    fn encode(
        &mut self,
        item: Request,
        destination: &mut bytes::BytesMut,
    ) -> Result<(), Self::Error> {
        self.requests.encode(item, destination)
    }
}

impl Decoder for ClientCodec {
    type Item = Response;
    type Error = CodecError;

    fn decode(&mut self, source: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.responses.decode(source)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("failed to connect to {address}: {source}")]
    Connect {
        address: SocketAddr,
        source: std::io::Error,
    },
    #[error("request timed out after {0:?}")]
    Timeout(Duration),
    #[error("failed to send a request: {0}")]
    Send(#[source] CodecError),
    #[error("connection closed before a response arrived")]
    Closed,
    #[error("failed to decode a response: {0}")]
    Receive(#[source] CodecError),
    #[error("server returned request identifier {actual}, expected {expected}")]
    RequestIdMismatch { expected: u64, actual: u64 },
    #[error("server returned protocol error {code}: {message}")]
    Protocol { code: u16, message: String },
    #[error("server rejected the deployment with code {code}: {message}")]
    Rejected { code: u16, message: String },
    #[error("server returned an unexpected response to {request}")]
    Unexpected { request: &'static str },
    #[error("server application '{actual}' does not match expected application '{expected}'")]
    ApplicationMismatch { expected: String, actual: String },
    #[error("request identifiers are exhausted")]
    RequestIdsExhausted,
}
