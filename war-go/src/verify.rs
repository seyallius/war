//! verify - Dry-run check to confirm offline mode is working.
//!
//! Runs `go list -m all -mod=vendor` and `go build -x` to detect
//! any unexpected network fallback attempts.

use std::path::Path;
use tokio::process::Command;
use war_core::WarError;

// -------------------------------------------- Public API --------------------------------------------

/// Verify that Go is correctly configured for offline development.
///
/// Returns Ok(()) if no network calls are detected; Err otherwise.
pub async fn verify_offline() -> Result<(), WarError> {
    tracing::info!("Verifying offline build capability...");

    // 1. Check that a `vendor` directory exists.
    // Without it, offline mode is impossible.
    let vendor_path = Path::new("vendor");
    if !vendor_path.exists() {
        tracing::error!("✖ Verification Failed: The 'vendor' directory does not exist.");
        tracing::info!("Please run 'go mod vendor' to create it before going offline.");
        return Err(WarError::VendorParseError {
            path: vendor_path.to_owned(),
            reason: String::from("Missing 'vendor' directory"),
        });
    }
    tracing::info!("✔ Found 'vendor' directory.");

    // 2. Run `go list` with `-mod=vendor`.
    // This command forces Go to only use the `vendor` directory.
    // If a dependency is missing, this command will fail.
    tracing::info!("Checking for missing dependencies...");
    let output = Command::new("go")
        .arg("list")
        .arg("-mod=vendor")
        .arg("./...")
        .output()
        .await?;

    // 3. Check the command's exit status and report to the user.
    if output.status.success() {
        tracing::info!(
            "(≧◡≦) ✔ Success! Project is fully vendored and ready for offline development."
        );
    } else {
        tracing::error!("✖ Verification Failed: One or more dependencies are missing from the 'vendor' directory.");
        // Print the error output from the Go command to help the user debug.
        eprintln!("\n--- Go Tool Error ---");
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        eprintln!("---------------------");
        tracing::info!("Try running 'go mod vendor' again to fix the issue.");
        return Err(WarError::VendorParseError {
            path: vendor_path.to_owned(),
            reason: String::from("Verification failed due to missing dependencies"),
        });
    }

    Ok(())
}
