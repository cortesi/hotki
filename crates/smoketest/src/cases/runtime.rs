//! Native runtime-health and recovery smoketest cases.

use std::fs;

use tracing::debug;

use super::ui::wait_for_notification_window;
use crate::{
    error::{Error, Result},
    session::{HotkiSession, HotkiSessionConfig},
    suite::CaseCtx,
};

/// Binding installed by the corrected activation config.
const ACTIVATION_IDENT: &str = "shift+cmd+r";
/// Binding expected after the app reconnects to a replacement server.
const RECONNECT_IDENT: &str = "shift+cmd+c";
/// Deliberately malformed source used to exercise initial config rejection.
const INVALID_CONFIG: &str = "return function(";

/// Render a minimal config containing one observable binding.
fn config_with_binding(ident: &str) -> String {
    format!(
        r#"
return function(menu, ctx)
  menu:bind("{ident}", "Runtime smoke", function(actx)
    actx:notify("info", "Runtime smoke", "ready")
  end)
end
"#
    )
}

/// Verify an invalid startup candidate is visible and a corrected candidate activates.
pub fn config_activation(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let config_path = ctx.scratch_path("invalid_initial_config.luau");
    let mut session: Option<HotkiSession> = None;

    ctx.setup(|ctx| {
        fs::write(&config_path, INVALID_CONFIG.as_bytes())?;
        session = Some(HotkiSession::spawn(
            HotkiSessionConfig::from_env()?
                .with_config(&config_path)
                .with_logs(true),
            ctx.run_budget(),
        )?);
        Ok(())
    })?;

    ctx.action(|ctx| {
        let session_ref = session.as_mut().ok_or_else(|| {
            Error::InvalidState("config activation session missing during action".into())
        })?;
        wait_for_notification_window(session_ref.pid() as i32, ctx.remaining_ms()?)?;

        fs::write(
            &config_path,
            config_with_binding(ACTIVATION_IDENT).as_bytes(),
        )?;
        let driver = session_ref.driver_mut();
        driver.activate_config(&config_path)?;
        driver.wait_for_idents(&[ACTIVATION_IDENT], ctx.remaining_ms()?)?;
        Ok(())
    })?;

    ctx.settle(|_| {
        let mut session_inner = session
            .take()
            .ok_or_else(|| Error::InvalidState("config activation session missing".into()))?;
        if let Err(err) = session_inner.shutdown() {
            debug!(?err, "server shutdown returned an error during cleanup");
        }
        session_inner.kill_and_wait();
        Ok(())
    })?;

    Ok(())
}

/// Verify the app reconnects and reinstalls its config after its server exits.
pub fn reconnect(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let config_path = ctx.scratch_path("reconnect_config.luau");
    let mut session: Option<HotkiSession> = None;

    ctx.setup(|ctx| {
        fs::write(
            &config_path,
            config_with_binding(RECONNECT_IDENT).as_bytes(),
        )?;
        session = Some(HotkiSession::spawn(
            HotkiSessionConfig::from_env()?
                .with_config(&config_path)
                .with_logs(true),
            ctx.run_budget(),
        )?);
        Ok(())
    })?;

    ctx.action(|ctx| {
        let session_ref = session
            .as_mut()
            .ok_or_else(|| Error::InvalidState("reconnect session missing during action".into()))?;
        let driver = session_ref.driver_mut();
        driver.wait_for_idents(&[RECONNECT_IDENT], ctx.remaining_ms()?)?;
        driver.shutdown()?;
        driver.ensure_ready(ctx.remaining_ms()?)?;
        driver.wait_for_idents(&[RECONNECT_IDENT], ctx.remaining_ms()?)?;
        Ok(())
    })?;

    ctx.settle(|_| {
        let mut session_inner = session
            .take()
            .ok_or_else(|| Error::InvalidState("reconnect session missing".into()))?;
        if let Err(err) = session_inner.shutdown() {
            debug!(
                ?err,
                "replacement server shutdown returned an error during cleanup"
            );
        }
        session_inner.kill_and_wait();
        Ok(())
    })?;

    Ok(())
}
