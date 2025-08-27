use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::process::Command;
use tokio::time::{sleep, Duration};

// This is a Unix-specific extension for pre_exec.
#[cfg(unix)]
use rlimit::{setrlimit, Resource};

pub mod calculator {
    tonic::include_proto!("calculator");
}

const MB: u64 = 1024 * 1024;

/// Manages a single plugin subprocess, restarting it with backoff on failure.
async fn plugin_manager_task(plugin_path: PathBuf, socket_path: PathBuf) {
    let plugin_name = plugin_path.file_name().unwrap().to_str().unwrap();
    let mut backoff = Duration::from_secs(100);
    const MAX_BACKOFF: Duration = Duration::from_secs(60);

    loop {
        println!("[supervisor] Starting plugin: {}", plugin_name);
        let mut cmd = Command::new(&plugin_path);
        cmd.arg(&socket_path);

        // Set resource limits before the process executes (Unix-only).
        #[cfg(unix)]
        unsafe {
            cmd.pre_exec(|| {
                // Set a 32 MB memory limit.
                setrlimit(Resource::AS, 16 * MB, 32 * MB)?;
                Ok(())
            });
        }

        let mut child = cmd.spawn().expect("Failed to spawn plugin");

        let status = child.wait().await.expect("Child process failed to wait");

        eprintln!(
            "[supervisor] Plugin '{}' exited with {}. Restarting in {:?}...",
            plugin_name, status, backoff
        );

        sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Makes a plugin call with exponential backoff retry logic
async fn make_plugin_call_with_retry(
    runtime_dir: &Path,
    plugin_name: &str,
    a: f64,
    b: f64,
) -> Result<f64, Box<dyn std::error::Error + Send + Sync>> {
    let mut retry_delay = Duration::from_millis(50);
    const MAX_RETRY_DELAY: Duration = Duration::from_secs(2);
    const MAX_RETRIES: u32 = 5;

    for attempt in 0..MAX_RETRIES {
        let socket_path = runtime_dir.join(format!("{}.sock", plugin_name));
        let endpoint =
            tonic::transport::Endpoint::from_shared(format!("unix:{}", socket_path.display()))?;

        match calculator::calculator_client::CalculatorClient::connect(endpoint).await {
            Ok(mut client) => {
                let request = tonic::Request::new(calculator::CalculationRequest { a, b });
                match client.calculate(request).await {
                    Ok(response) => {
                        if attempt > 0 {
                            println!("[supervisor-test] SUCCESS on retry attempt {} for {}", attempt + 1, plugin_name);
                        }
                        return Ok(response.into_inner().result);
                    }
                    Err(e) => {
                        if attempt < MAX_RETRIES - 1 {
                            eprintln!(
                                "[supervisor-test] {} call failed (attempt {}), retrying in {:?}: {}",
                                plugin_name,
                                attempt + 1,
                                retry_delay,
                                e
                            );
                            sleep(retry_delay).await;
                            retry_delay = (retry_delay * 2).min(MAX_RETRY_DELAY);
                        } else {
                            return Err(format!("{} call failed after {} attempts: {}", plugin_name, MAX_RETRIES, e).into());
                        }
                    }
                }
            }
            Err(e) => {
                if attempt < MAX_RETRIES - 1 {
                    eprintln!(
                        "[supervisor-test] {} connection failed (attempt {}), retrying in {:?}: {}",
                        plugin_name,
                        attempt + 1,
                        retry_delay,
                        e
                    );
                    sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(MAX_RETRY_DELAY);
                } else {
                    return Err(format!("{} connection failed after {} attempts: {}", plugin_name, MAX_RETRIES, e).into());
                }
            }
        }
    }

    Err("Unexpected: reached end of retry loop".into())
}

#[tokio::main]
async fn main() {
    let runtime_dir = Path::new("/tmp/grpc_plugins_demo");
    std::fs::create_dir_all(runtime_dir).unwrap();

    // Discover plugins from the target directory.
    let plugins_to_run = ["adder", "subtract", "divide", "multiply", "modulo"];
    let mut plugin_handles = Vec::new();

    let loading_start = Instant::now();

    for name in plugins_to_run {
        // This relies on `cargo build` placing the binaries in a predictable location.
        if let Ok(plugin_path) = std::fs::canonicalize(format!("./target/debug/{}", name)) {
            println!("[supervisor] Found plugin: {}", plugin_path.display());
            let socket_path = runtime_dir.join(format!("{}.sock", name));
            let handle = tokio::spawn(plugin_manager_task(plugin_path, socket_path));
            plugin_handles.push(handle);
        }
    }

    let loading_duration = loading_start.elapsed();
    println!("\n[supervisor] All plugin managers started in {:?}\n", loading_duration);

    if plugin_handles.is_empty() {
        eprintln!("No plugins found. Did you run `cargo build` first?");
        return;
    }

    let startup_start = Instant::now();
    println!("\n[supervisor] All plugin managers started. Waiting for plugins to become available...\n");
    
    // Wait for all plugins to become available by testing them
    let mut plugins_ready = std::collections::HashSet::new();
    while plugins_ready.len() < plugins_to_run.len() {
        for &plugin_name in &plugins_to_run {
            if !plugins_ready.contains(plugin_name) {
                // Try to make a quick test call to see if plugin is ready
                match make_plugin_call_with_retry(runtime_dir, plugin_name, 1.0, 1.0).await {
                    Ok(_) => {
                        println!("[supervisor] Plugin '{}' is ready", plugin_name);
                        plugins_ready.insert(plugin_name);
                    }
                    Err(_) => {
                        // Plugin not ready yet, continue waiting
                    }
                }
            }
        }
        if plugins_ready.len() < plugins_to_run.len() {
            sleep(Duration::from_millis(50)).await;
        }
    }
    
    let startup_duration = startup_start.elapsed();
    println!("\n[supervisor] All {} plugins started and ready in {:?}\n", plugins_to_run.len(), startup_duration);
    println!("[supervisor] Now beginning continuous testing...\n");

    // Main loop to periodically test the running plugins.
    let mut plugin_index = 0;
    loop {
        sleep(Duration::from_millis(100)).await; // 10 calls per second
        let a = rand::random::<f64>() * 10.0;
        let b = rand::random::<f64>() * 10.0;

        // Rotate through all plugins
        let plugin_name = plugins_to_run[plugin_index % plugins_to_run.len()];
        let operation_symbol = match plugin_name {
            "adder" => "+",
            "subtract" => "-",
            "divide" => "/",
            "multiply" => "*",
            "modulo" => "%",
            _ => "?",
        };

        // Attempt the call with exponential backoff retry
        match make_plugin_call_with_retry(runtime_dir, plugin_name, a, b).await {
            Ok(result) => {
                // Assert that the result is mathematically correct
                let expected = match plugin_name {
                    "adder" => a + b,
                    "subtract" => a - b,
                    "divide" => {
                        if b != 0.0 { a / b } else { 
                            // For division by zero, just skip the assertion
                            println!("[supervisor-test] Division by zero detected, skipping assertion");
                            result
                        }
                    },
                    "multiply" => a * b,
                    "modulo" => a % b,
                    _ => {
                        eprintln!("[supervisor-test] Unknown plugin: {}", plugin_name);
                        result
                    }
                };
                
                // Use epsilon comparison for floating point numbers
                let epsilon = 1e-10;
                if plugin_name != "divide" || b != 0.0 {
                    assert!(
                        (result - expected).abs() < epsilon,
                        "Plugin {} returned incorrect result: expected {}, got {} (inputs: {} {} {})",
                        plugin_name, expected, result, a, operation_symbol, b
                    );
                }
                
                println!(
                    "[supervisor-test] SUCCESS: {} {} {} = {} ({}) - VERIFIED",
                    a, operation_symbol, b, result, plugin_name
                );
            }
            Err(e) => {
                eprintln!(
                    "[supervisor-test] FAILED after all retries: {}",
                    e
                );
            }
        }

        plugin_index += 1;
    }
}
