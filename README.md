# envop

CLI en Rust que sube las variables de un archivo `.env` a un item de 1Password
usando el CLI oficial [`op`](https://developer.1password.com/docs/cli/). Un campo
*concealed* por variable, con **upsert** idempotente y generación opcional de una
plantilla con referencias `op://`.

Pensado para un flujo **zero-secrets-on-disk**: los secretos viven solo en 1Password
y las apps los consumen con `op run` / `op inject`, sin dejar el `.env` en claro.

## Requisitos

- [`op` CLI](https://developer.1password.com/docs/cli/get-started/) 2.x instalado y con sesión (`op signin`).
- Rust/Cargo para compilar.

## Instalación

Linux / macOS (detecta OS/arquitectura y baja el binario de la última release):

```bash
curl -fsSL https://raw.githubusercontent.com/RomanshkVolkov/envop/main/install.sh | sh
```

O compilando desde el código:

```bash
cargo install --path .        # instala en ~/.cargo/bin
# o: cargo build --release     → target/release/envop
```

## Publicar una release

El workflow [`release.yml`](.github/workflows/release.yml) se dispara al hacer push
de un tag `v*`, compila para Linux y macOS (x86_64 y arm64) y sube los binarios
a la GitHub Release. **Sube antes la versión en `Cargo.toml`** para que coincida con
el tag (un job lo verifica y falla si no cuadran):

```bash
# 1) edita `version` en Cargo.toml (p. ej. 0.2.0)
git commit -am "release: v0.2.0"
git tag v0.2.0
git push origin main --tags
```

## Uso

```bash
envop push <FILE> --vault <VAULT> [--item <ITEM>] [opciones]
```

| Opción | Descripción |
| --- | --- |
| `<FILE>` | Ruta del `.env` (por defecto `.env`). |
| `-v, --vault` | Vault de 1Password de destino. **(requerido)** |
| `-i, --item` | Título del item. Si se omite, se pregunta por consola. |
| `-a, --account` | Cuenta de 1Password (email, URL, shorthand o account ID). |
| `-c, --category` | Categoría al crear el item (por defecto `Secure Note`). |
| `--tpl <PATH>` | Escribe una plantilla `.env` con referencias `op://` en **otro** archivo. |
| `--in-place` | Reescribe el **propio** `.env`: cambia cada valor por su referencia `op://` (conserva comentarios y secciones). |
| `--dry-run` | Muestra qué haría, con los valores enmascarados. |

### Ejemplos

```bash
# Subir .env al vault Dev (pregunta el nombre del item)
envop push .env --vault Dev

# Explícito, y generar la plantilla de referencias
envop push .env --vault Dev --item myproj --tpl .env.tpl

# Previsualizar sin tocar 1Password
envop push .env --vault Dev --item myproj --dry-run
```

## Idempotencia (upsert)

Si el item no existe se crea (`op item create`); si existe, se actualizan sus campos
(`op item edit`, que añade los que falten). Re-ejecutar es seguro.

> Nota: el upsert **no borra** variables que hayas quitado del `.env` (quedan en el
> item). Elimínalas a mano en 1Password si hace falta.

## Secciones

Un comentario con la forma `# [Nombre]` abre una sección; las variables siguientes
van a esa sección del item. Un `# comentario` normal no la rompe, y `# []` la resetea.

```dotenv
API_URL=https://api.example.com     # sin sección

# [Database]
DB_HOST=localhost
DB_PORT=5432

# []
LOG_LEVEL=info                       # vuelve a sin sección
```

Las claves deben ser **SNAKE_CASE** (`A-Z0-9_`, sin empezar por dígito).

## Varias cuentas

`op` soporta varias cuentas en la misma máquina. `envop` lo gestiona así:

- Con `--account <id>` → usa esa cuenta.
- Sin `--account` y **una** cuenta → usa la de por defecto.
- Sin `--account` y **2+** cuentas → si hay terminal, te pregunta cuál; si no
  (CI/pipe), aborta listándolas para que pases `--account`.

```bash
op account list   # ver los identificadores disponibles
```

## Plantilla `op://` y consumo sin secretos en disco

Con `--tpl` se genera un `.env` de plantilla (en otro archivo) donde cada valor es una
referencia. Con `--in-place` se reescribe el propio `.env` (conservando comentarios y
marcadores `# [Sección]`). En ambos casos:

```dotenv
DB_HOST=op://Dev/myproj/Database/DB_HOST
API_URL=op://Dev/myproj/API_URL
```

> `envop` se niega a subir un `.env` que ya contiene referencias `op://`, para no
> pisar el item con las referencias en vez de los valores reales.

Consúmela sin materializar secretos en disco:

```bash
op run --env-file .env.tpl -- npm start   # inyecta en memoria del proceso
op inject -i .env.tpl -o .env             # si necesitas el archivo en claro
```

## Formato `.env` soportado

- Comentarios (`#`) y líneas en blanco.
- Prefijo opcional `export`.
- Comillas dobles con escapes (`\n`, `\t`, `\r`, `\"`, `\\`) y comillas simples literales.
- Comentario inline en valores sin comillas (`FOO=bar # nota`).
- Valores con `=` (URLs, DSNs, etc.).

## Diseño

`envop` no implementa cripto ni la API de 1Password: es un orquestador fino que
delega auth, sesión y almacenamiento en `op`.

```
src/
├── main.rs        # CLI (clap), comando push, prompts, plantilla op://
├── env_parser.rs  # parser .env + secciones (con tests)
└── op.rs          # envoltura sobre `op` (cuenta, exists, create/edit)
```

```bash
cargo test   # tests del parser
```
