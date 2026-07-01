//! envs-to-op — sube un archivo `.env` a un item de 1Password vía el CLI `op`.
//!
//! Un campo *concealed* por variable, con upsert idempotente, y generación
//! opcional de una plantilla `.env` con referencias `op://vault/item/KEY`.

mod env_parser;
mod op;

use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "envop", version, about = "Sube variables de un .env a 1Password (op CLI)")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Sube (upsert) las variables de un .env a un item de 1Password.
    Push(PushArgs),
}

#[derive(Parser)]
struct PushArgs {
    /// Ruta del archivo .env a leer.
    #[arg(default_value = ".env")]
    file: PathBuf,

    /// Cuenta de 1Password (email, dirección de acceso, shorthand o account ID).
    /// Útil si tienes varias cuentas; se reenvía como `op --account`.
    #[arg(short, long)]
    account: Option<String>,

    /// Vault de 1Password de destino.
    #[arg(short, long)]
    vault: String,

    /// Título del item de 1Password (se crea si no existe, se actualiza si existe).
    /// Si se omite, se pregunta por consola.
    #[arg(short, long)]
    item: Option<String>,

    /// Categoría del item al crearlo.
    #[arg(short, long, default_value = "Secure Note")]
    category: String,

    /// Muestra qué haría sin ejecutar `op` (valores enmascarados).
    #[arg(long)]
    dry_run: bool,

    /// Escribe una plantilla con referencias op://vault/item/KEY en esta ruta.
    #[arg(long, value_name = "PATH")]
    tpl: Option<PathBuf>,

    /// Reescribe el propio .env reemplazando cada valor por su referencia op://
    /// (conserva comentarios y marcadores de sección). Solo tras subir con éxito.
    #[arg(long)]
    in_place: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Push(args) => push(args),
    }
}

fn push(args: PushArgs) -> Result<()> {
    let content = fs::read_to_string(&args.file)
        .with_context(|| format!("no se pudo leer {}", args.file.display()))?;
    let vars = env_parser::parse(&content)?;
    if vars.is_empty() {
        anyhow::bail!("{} no contiene variables", args.file.display());
    }
    if vars.iter().any(|v| v.value.starts_with("op://")) {
        anyhow::bail!(
            "{} ya contiene referencias op:// (¿lo reescribiste con --in-place?). \
             Usa el .env con los valores reales para subirlo.",
            args.file.display()
        );
    }

    let item = match args.item {
        Some(i) => i,
        None => prompt("Nombre del item de 1Password: ")?,
    };

    // Resuelve la cuenta: si no se indicó y hay 2+ configuradas, exige elegir.
    let account = resolve_account(args.account)?;
    let op = op::Op::new(account.clone());

    if args.dry_run {
        let exists = op.check_session().is_ok() && op.item_exists(&args.vault, &item).unwrap_or(false);
        let (action, category) = if exists {
            ("edit", None)
        } else {
            ("create", Some(args.category.as_str()))
        };
        println!("[dry-run] {} variable(s) desde {}", vars.len(), args.file.display());
        println!("[dry-run] {}", op::preview(action, account.as_deref(), &args.vault, &item, category, &vars));
        if let Some(tpl) = &args.tpl {
            println!("[dry-run] escribiría plantilla en {}", tpl.display());
        }
        if args.in_place {
            println!("[dry-run] reescribiría {} con referencias op:// (in-place)", args.file.display());
        }
        return Ok(());
    }

    op.check_session()?;

    if op.item_exists(&args.vault, &item)? {
        println!("Item «{}» existe → actualizando {} campo(s)…", item, vars.len());
        op.edit_item(&args.vault, &item, &vars)?;
    } else {
        println!("Item «{}» no existe → creándolo con {} campo(s)…", item, vars.len());
        op.create_item(&args.vault, &item, &args.category, &vars)?;
    }
    println!("✓ Subido a 1Password (vault «{}»).", args.vault);

    if let Some(tpl_path) = &args.tpl {
        write_template(tpl_path, &args.vault, &item, &vars)?;
        println!("✓ Plantilla de referencias op:// escrita en {}", tpl_path.display());
    }

    if args.in_place {
        let rewritten = to_references(&content, &args.vault, &item);
        fs::write(&args.file, rewritten)
            .with_context(|| format!("no se pudo reescribir {}", args.file.display()))?;
        println!("✓ {} reescrito con referencias op://", args.file.display());
    }

    Ok(())
}

/// Reescribe el contenido de un `.env` reemplazando el valor de cada variable por su
/// referencia `op://vault/item/[section/]KEY`, conservando líneas en blanco, comentarios
/// y marcadores de sección `# [Nombre]` para que las secciones sobrevivan a re-ejecuciones.
fn to_references(content: &str, vault: &str, item: &str) -> String {
    let mut out = String::new();
    let mut section: Option<String> = None;

    for raw in content.lines() {
        let trimmed = raw.trim_start();

        // Comentarios y líneas en blanco se copian tal cual (y actualizan la sección actual).
        if trimmed.is_empty() {
            out.push_str(raw);
            out.push('\n');
            continue;
        }
        if let Some(body) = trimmed.strip_prefix('#') {
            if let Some(inner) = body.trim().strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                let name = inner.trim();
                section = if name.is_empty() { None } else { Some(name.to_string()) };
            }
            out.push_str(raw);
            out.push('\n');
            continue;
        }

        // Línea de variable: preserva indentación y prefijo `export`, sustituye el valor.
        let indent = &raw[..raw.len() - trimmed.len()];
        let (export_prefix, rest) = match trimmed.strip_prefix("export ") {
            Some(r) => ("export ", r.trim_start()),
            None => ("", trimmed),
        };
        match rest.split_once('=') {
            Some((key_raw, val_raw)) => {
                let key = key_raw.trim();
                let reference = match &section {
                    Some(s) => format!("op://{vault}/{item}/{s}/{key}"),
                    None => format!("op://{vault}/{item}/{key}"),
                };
                let comment = trailing_comment(val_raw);
                out.push_str(&format!("{indent}{export_prefix}{key}={reference}{comment}\n"));
            }
            None => {
                out.push_str(raw);
                out.push('\n');
            }
        }
    }

    out
}

/// Devuelve el comentario inline que sigue al valor (con su espacio original, p. ej.
/// ` # nota`), o "" si no hay. Respeta comillas: un `#` dentro de comillas no es comentario.
fn trailing_comment(val_raw: &str) -> String {
    let v = val_raw.trim_start();

    if let Some(rest) = v.strip_prefix('"') {
        let mut chars = rest.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => {
                    chars.next(); // salta el carácter escapado
                }
                '"' => return rest[i + 1..].to_string(),
                _ => {}
            }
        }
        return String::new(); // comilla sin cerrar
    }

    if let Some(rest) = v.strip_prefix('\'') {
        return match rest.find('\'') {
            Some(pos) => rest[pos + 1..].to_string(),
            None => String::new(),
        };
    }

    // Sin comillas: el comentario empieza en el primer ` #`.
    match v.find(" #") {
        Some(pos) => v[pos..].to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::to_references;

    #[test]
    fn reescribe_conservando_comentarios_y_secciones() {
        let src = "# cabecera\nexport API_URL=https://x.com # nota\n\n# [Database]\nDB_HOST=localhost\n";
        let got = to_references(src, "Dev", "myproj");
        let want = "# cabecera\nexport API_URL=op://Dev/myproj/API_URL # nota\n\n# [Database]\nDB_HOST=op://Dev/myproj/Database/DB_HOST\n";
        assert_eq!(got, want);
    }

    #[test]
    fn preserva_comentario_inline_incluso_con_comillas() {
        // El `#` dentro de comillas no es comentario; el de fuera sí se conserva.
        let src = "A=\"val # dentro\" # fuera\nB=plain\n";
        let got = to_references(src, "V", "I");
        assert_eq!(got, "A=op://V/I/A # fuera\nB=op://V/I/B\n");
    }
}

/// Decide qué cuenta usar. Si el usuario la indicó, la respeta. Si no, consulta
/// `op`: con 0-1 cuentas deja que `op` use la de por defecto; con 2+ exige elegir.
fn resolve_account(explicit: Option<String>) -> Result<Option<String>> {
    if explicit.is_some() {
        return Ok(explicit);
    }
    let accounts = op::Op::new(None).list_accounts()?;
    if accounts.len() <= 1 {
        return Ok(None); // 0-1 cuentas: deja que `op` use la de por defecto.
    }

    // Varias cuentas y ninguna indicada. Si hay terminal, pregunta cuál; si no, aborta.
    if !io::stdin().is_terminal() {
        let listed = accounts
            .iter()
            .map(|a| format!("  - {}  ({})", a.email, a.url))
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!(
            "tienes {} cuentas de 1Password; indica cuál con --account <email|url|shorthand>:\n{listed}",
            accounts.len()
        );
    }

    println!("Tienes varias cuentas de 1Password. ¿Cuál usar?");
    for (i, a) in accounts.iter().enumerate() {
        println!("  {}) {}  ({})", i + 1, a.email, a.url);
    }
    loop {
        print!("Número [1-{}]: ", accounts.len());
        io::stdout().flush().ok();
        let mut line = String::new();
        if io::stdin().read_line(&mut line).context("no se pudo leer de stdin")? == 0 {
            anyhow::bail!("entrada cancelada; no se eligió cuenta");
        }
        match line.trim().parse::<usize>() {
            Ok(n) if (1..=accounts.len()).contains(&n) => {
                let a = &accounts[n - 1];
                // La URL (dirección de acceso) es un identificador canónico que `op --account` acepta.
                let id = if a.url.is_empty() { a.email.clone() } else { a.url.clone() };
                return Ok(Some(id));
            }
            _ => eprintln!("Opción inválida, elige un número entre 1 y {}.", accounts.len()),
        }
    }
}

/// Pregunta por consola y devuelve la respuesta recortada; error si queda vacía.
fn prompt(msg: &str) -> Result<String> {
    print!("{msg}");
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin().read_line(&mut line).context("no se pudo leer de stdin")?;
    let line = line.trim().to_string();
    if line.is_empty() {
        anyhow::bail!("nombre del item vacío");
    }
    Ok(line)
}

/// Genera un `.env` de plantilla donde cada valor es una referencia `op://`.
/// Úsalo con `op inject -i <tpl> -o .env` o `op run`.
fn write_template(path: &PathBuf, vault: &str, item: &str, vars: &[env_parser::EnvVar]) -> Result<()> {
    let mut out = String::from("# Generado por envs-to-op. Usa: op inject -i este-archivo -o .env\n");
    for v in vars {
        let reference = match &v.section {
            Some(s) => format!("op://{}/{}/{}/{}", vault, item, s, v.key),
            None => format!("op://{}/{}/{}", vault, item, v.key),
        };
        out.push_str(&format!("{}={}\n", v.key, reference));
    }
    fs::write(path, out).with_context(|| format!("no se pudo escribir {}", path.display()))?;
    Ok(())
}
