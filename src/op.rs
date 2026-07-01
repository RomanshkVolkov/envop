//! Envoltura fina sobre el CLI `op` de 1Password.
//!
//! No implementamos cripto ni la API: delegamos auth, sesión y almacenamiento en `op`.
//! `Op` lleva la cuenta seleccionada (`--account`) y la reenvía a cada invocación,
//! para soportar varias cuentas de 1Password en la misma máquina.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::process::Command;

use crate::env_parser::EnvVar;

/// Una cuenta de 1Password tal como la reporta `op account list --format=json`.
#[derive(Deserialize)]
pub struct Account {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub email: String,
}

/// Contexto de ejecución de `op` con la cuenta opcional ya fijada.
pub struct Op {
    account: Option<String>,
}

impl Op {
    pub fn new(account: Option<String>) -> Self {
        Self { account }
    }

    /// Crea un `Command` de `op` con `--account <cuenta>` inyectado si procede.
    fn cmd(&self) -> Command {
        let mut c = Command::new("op");
        if let Some(a) = &self.account {
            c.args(["--account", a]);
        }
        c
    }

    /// Lista las cuentas configuradas en `op`. Devuelve vacío si no hay ninguna.
    pub fn list_accounts(&self) -> Result<Vec<Account>> {
        let out = self
            .cmd()
            .args(["account", "list", "--format=json"])
            .output()
            .context("no se pudo ejecutar `op account list` (¿está instalado el CLI de 1Password?)")?;
        if !out.status.success() {
            return Ok(vec![]);
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let trimmed = stdout.trim();
        if trimmed.is_empty() {
            return Ok(vec![]);
        }
        serde_json::from_str(trimmed).context("no se pudo parsear la salida de `op account list`")
    }

    /// Comprueba que `op` está instalado y hay sesión activa para la cuenta (`op whoami`).
    pub fn check_session(&self) -> Result<()> {
        let out = self
            .cmd()
            .arg("whoami")
            .output()
            .context("no se pudo ejecutar `op` (¿está instalado el CLI de 1Password?)")?;
        if !out.status.success() {
            let hint = match &self.account {
                Some(a) => format!("`op signin --account {a}`"),
                None => "`op signin` (o especifica --account)".to_string(),
            };
            bail!(
                "no hay sesión de 1Password activa. Ejecuta {hint}.\n{}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    /// ¿Existe ya el item en el vault dado?
    pub fn item_exists(&self, vault: &str, item: &str) -> Result<bool> {
        let out = self
            .cmd()
            .args(["item", "get", item, "--vault", vault])
            .output()
            .context("fallo ejecutando `op item get`")?;
        Ok(out.status.success())
    }

    /// Crea un item nuevo con un campo concealed por variable.
    pub fn create_item(&self, vault: &str, item: &str, category: &str, vars: &[EnvVar]) -> Result<()> {
        let mut args = vec![
            "item".to_string(),
            "create".to_string(),
            "--category".to_string(),
            category.to_string(),
            "--title".to_string(),
            item.to_string(),
            "--vault".to_string(),
            vault.to_string(),
        ];
        args.extend(field_assignments(vars));
        self.run("crear el item", &args)
    }

    /// Actualiza (upsert) los campos de un item existente; `op item edit` añade los que falten.
    pub fn edit_item(&self, vault: &str, item: &str, vars: &[EnvVar]) -> Result<()> {
        let mut args = vec![
            "item".to_string(),
            "edit".to_string(),
            item.to_string(),
            "--vault".to_string(),
            vault.to_string(),
        ];
        args.extend(field_assignments(vars));
        self.run("editar el item", &args)
    }

    fn run(&self, what: &str, args: &[String]) -> Result<()> {
        let status = self
            .cmd()
            .args(args)
            .status()
            .with_context(|| format!("fallo al {what} con `op`"))?;
        if !status.success() {
            bail!("`op` falló al {what} (código {:?})", status.code());
        }
        Ok(())
    }
}

/// Construye las asignaciones de campo `[section.]KEY[password]=value` para cada variable.
fn field_assignments(vars: &[EnvVar]) -> Vec<String> {
    vars.iter()
        .map(|v| match &v.section {
            Some(s) => format!("{}.{}[password]={}", s, v.key, v.value),
            None => format!("{}[password]={}", v.key, v.value),
        })
        .collect()
}

/// Representación imprimible del comando `op` que se ejecutaría (para `--dry-run`).
/// Enmascara los valores para no filtrar secretos por pantalla.
pub fn preview(
    action: &str,
    account: Option<&str>,
    vault: &str,
    item: &str,
    category: Option<&str>,
    vars: &[EnvVar],
) -> String {
    let mut parts = vec!["op".to_string()];
    if let Some(a) = account {
        parts.push(format!("--account {a:?}"));
    }
    parts.push("item".to_string());
    parts.push(action.to_string());
    match category {
        Some(cat) => {
            parts.push(format!("--category {cat:?}"));
            parts.push(format!("--title {item:?}"));
        }
        None => parts.push(format!("{item:?}")),
    }
    parts.push(format!("--vault {vault:?}"));
    for v in vars {
        match &v.section {
            Some(s) => parts.push(format!("'{}.{}[password]=********'", s, v.key)),
            None => parts.push(format!("'{}[password]=********'", v.key)),
        }
    }
    parts.join(" ")
}
