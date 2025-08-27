use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::{transport::Server, Request, Response, Status};

// Import the generated gRPC code
pub mod calculator {
    tonic::include_proto!("calculator");
}
use calculator::{
    calculator_server::{Calculator, CalculatorServer},
    CalculationRequest, CalculationResponse,
};

#[derive(Debug, Default)]
pub struct MyCalculator {}

#[tonic::async_trait]
impl Calculator for MyCalculator {
    async fn calculate(
        &self,
        request: Request<CalculationRequest>,
    ) -> Result<Response<CalculationResponse>, Status> {
        let req = request.into_inner();

        // --- THIS IS OUR RANDOM CRASH FOR TESTING ---
        if rand::random::<f64>() > 0.9 {
            println!("[divide-plugin] Ahh! I'm crashing!");
            panic!("A random, unhandled error occurred!");
        }

        let result = req.a / req.b;
        println!(
            "[divide-plugin] Calculated {} / {} = {}",
            req.a, req.b, result
        );

        Ok(Response::new(CalculationResponse { result }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The supervisor will pass the socket path as the first argument
    let socket_path = std::env::args().nth(1).expect("Usage: divide <socket_path>");

    // Clean up any old socket file before binding
    if std::fs::metadata(&socket_path).is_ok() {
        std::fs::remove_file(&socket_path)?;
    }

    let uds = UnixListener::bind(&socket_path)?;
    let uds_stream = UnixListenerStream::new(uds);
    let calculator = MyCalculator::default();

    println!("[divide-plugin] Server listening on socket: {}", socket_path);

    Server::builder()
        .add_service(CalculatorServer::new(calculator))
        .serve_with_incoming(uds_stream)
        .await?;

    Ok(())
}
