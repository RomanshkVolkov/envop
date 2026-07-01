//! Parser mínimo pero robusto de archivos `.env`.
//!
//! Soporta:
//! - Líneas en blanco y comentarios (`#`).
//! - Marcador de sección `# [Nombre]`: las variables siguientes van a esa sección
//!   del item de 1Password (hasta el próximo marcador; `# []` la resetea).
//! - Prefijo opcional `export`.
//! - Valores entre comillas simples (literales) y dobles (con escapes `\n`, `\t`, `\"`, `\\`).
//! - Comentarios inline en valores sin comillas (precedidos por espacio: `FOO=bar # nota`).
//! - Espacios alrededor del `=` y del valor sin comillas.

use anyhow::{bail, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
    /// Sección del item de 1Password a la que pertenece (None = sin sección).
    pub section: Option<String>,
}

/// Parsea el contenido de un `.env` en pares clave/valor, preservando el orden.
/// Devuelve error si una línea no vacía y sin comentario no tiene la forma `KEY=VALUE`
/// o la clave es inválida.
pub fn parse(content: &str) -> Result<Vec<EnvVar>> {
    let mut out = Vec::new();
    let mut section: Option<String> = None;

    for (idx, raw) in content.lines().enumerate() {
        let lineno = idx + 1;
        let line = raw.trim_start();

        if line.is_empty() {
            continue;
        }

        // Comentario: puede ser un marcador de sección `# [Nombre]` o un comentario normal.
        if let Some(body) = line.strip_prefix('#') {
            let body = body.trim();
            if let Some(inner) = body.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                let name = inner.trim();
                section = if name.is_empty() { None } else { Some(name.to_string()) };
            }
            continue;
        }

        // Prefijo `export ` opcional.
        let line = line.strip_prefix("export ").map(str::trim_start).unwrap_or(line);

        let Some((key_raw, rest)) = line.split_once('=') else {
            bail!("línea {lineno}: se esperaba KEY=VALUE, encontrado: {raw:?}");
        };

        let key = key_raw.trim();
        validate_key(key, lineno)?;

        let value = parse_value(rest, lineno)?;
        out.push(EnvVar { key: key.to_string(), value, section: section.clone() });
    }

    Ok(out)
}

fn validate_key(key: &str, lineno: usize) -> Result<()> {
    if key.is_empty() {
        bail!("línea {lineno}: clave vacía");
    }
    let ok = key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !ok || key.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(true) {
        bail!("línea {lineno}: clave inválida {key:?} (usa SNAKE_CASE: letras, dígitos y `_`, sin empezar por dígito)");
    }
    Ok(())
}

fn parse_value(rest: &str, lineno: usize) -> Result<String> {
    let rest = rest.trim_start();

    // Comilla doble: interpretar escapes hasta la comilla de cierre.
    if let Some(inner) = rest.strip_prefix('"') {
        let mut value = String::new();
        let mut chars = inner.chars();
        loop {
            match chars.next() {
                Some('"') => return Ok(value),
                Some('\\') => match chars.next() {
                    Some('n') => value.push('\n'),
                    Some('t') => value.push('\t'),
                    Some('r') => value.push('\r'),
                    Some('"') => value.push('"'),
                    Some('\\') => value.push('\\'),
                    Some(other) => {
                        value.push('\\');
                        value.push(other);
                    }
                    None => bail!("línea {lineno}: escape colgante al final del valor"),
                },
                Some(c) => value.push(c),
                None => bail!("línea {lineno}: falta la comilla doble de cierre"),
            }
        }
    }

    // Comilla simple: literal, sin escapes, hasta la comilla de cierre.
    if let Some(inner) = rest.strip_prefix('\'') {
        return match inner.split_once('\'') {
            Some((value, _trailing)) => Ok(value.to_string()),
            None => bail!("línea {lineno}: falta la comilla simple de cierre"),
        };
    }

    // Sin comillas: corta en el primer ` #` (comentario inline) y recorta espacios.
    let value = match rest.find(" #") {
        Some(pos) => &rest[..pos],
        None => rest,
    };
    Ok(value.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(k: &str, v: &str) -> EnvVar {
        EnvVar { key: k.into(), value: v.into(), section: None }
    }

    fn kvs(k: &str, v: &str, s: &str) -> EnvVar {
        EnvVar { key: k.into(), value: v.into(), section: Some(s.into()) }
    }

    #[test]
    fn parsea_basico_comentarios_y_export() {
        let out = parse("# comentario\nexport FOO=bar\n\nBAZ=qux # inline\n").unwrap();
        assert_eq!(out, vec![kv("FOO", "bar"), kv("BAZ", "qux")]);
    }

    #[test]
    fn comillas_dobles_con_escapes_y_hash() {
        let out = parse("A=\"line1\\nline2\"\nB=\"val # no-comment\"\n").unwrap();
        assert_eq!(out, vec![kv("A", "line1\nline2"), kv("B", "val # no-comment")]);
    }

    #[test]
    fn comillas_simples_literales() {
        let out = parse("A='no\\nescape'\n").unwrap();
        assert_eq!(out, vec![kv("A", "no\\nescape")]);
    }

    #[test]
    fn valor_con_signos_igual() {
        let out = parse("URL=postgres://u:p@h/db?x=1\n").unwrap();
        assert_eq!(out, vec![kv("URL", "postgres://u:p@h/db?x=1")]);
    }

    #[test]
    fn secciones_por_marcador_y_reset() {
        let src = "GLOBAL=1\n# [Database]\nDB_HOST=localhost\n# comentario normal\nDB_PORT=5432\n# []\nOTRO=x\n";
        let out = parse(src).unwrap();
        assert_eq!(
            out,
            vec![
                kv("GLOBAL", "1"),
                kvs("DB_HOST", "localhost", "Database"),
                kvs("DB_PORT", "5432", "Database"),
                kv("OTRO", "x"),
            ]
        );
    }

    #[test]
    fn clave_invalida_falla() {
        assert!(parse("1BAD=x\n").is_err());
        assert!(parse("sin_igual\n").is_err());
        assert!(parse("FOO.BAR=x\n").is_err()); // sin puntos: SNAKE_CASE
    }
}
