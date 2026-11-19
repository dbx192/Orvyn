<p align="center">
  <img src="packaging/icons/icon.png" width="96" alt="Orvyn">
</p>
<h1 align="center">Orvyn</h1>

<p align="center">
  <b>Una potente herramienta de bases de datos. La velocidad nativa de Rust.</b><br>
  35 fuentes de datos. SQL, NoSQL, búsqueda y análisis en un solo espacio de trabajo.
</p>

<p align="center">
  <a href="https://github.com/dbx192/Orvyn/releases/latest"><img src="https://img.shields.io/github/v/tag/dbx192/Orvyn?label=release&amp;color=teal" alt="Última versión"></a>
  <img src="https://img.shields.io/badge/built_with-Rust-orange.svg" alt="Desarrollado en Rust">
  <img src="https://img.shields.io/badge/desktop-Windows%20%7C%20Linux%20%7C%20macOS-0078D4.svg" alt="Windows, Linux, macOS">
  <img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="Licencia MIT">
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · <a href="README.hi.md">हिन्दी</a> · <b>Español</b> · <a href="README.fr.md">Français</a>
</p>

---

**Orvyn es un entorno de alto rendimiento para trabajar con bases de datos, desarrollado en Rust con egui.** Escribe SQL, edita registros, diseña esquemas, administra claves Redis, consulta documentos MongoDB, explora índices de búsqueda y exporta tablas grandes desde una aplicación de escritorio nativa.

Desde bases de datos de producción hasta análisis locales con DuckDB: controladores de protocolo nativo, integraciones HTTP y puentes ODBC conectan **35 tipos de fuentes de datos**. SQLite viene integrado y los paquetes de escritorio incluyen DuckDB 1.5.4.

**Ejecución nativa. Tablas virtualizadas. Exportación en streaming. Un espacio de trabajo sin fronteras.**

## Rendimiento desde la arquitectura

| Tecnología | Ventaja |
| --- | --- |
| **Rust + egui nativos** | Aplicación compilada, sin Electron ni motor de navegador. |
| **Tokio en segundo plano** | Tareas de base de datos fuera del hilo de interfaz y un grupo de trabajo con concurrencia limitada para operaciones bloqueantes. |
| **Tablas virtualizadas** | Las vistas de resultados y datos dibujan solo las filas visibles. |
| **Paginación, filtros y ordenación en el servidor** | Consulta páginas acotadas y envía los filtros de tabla a la base de datos. |
| **Conexiones reutilizables** | Los controladores MySQL y PostgreSQL mantienen grupos de conexiones. |
| **Streaming en lotes de 5.000 filas** | Los controladores de protocolo MySQL/PostgreSQL cuentan una vez y recorren los datos una sola vez, sin pasar por la paginación del editor ni su historial. |
| **Escritura incremental** | CSV, JSON, TSV y XLSX se escriben progresivamente; XLSX utiliza hojas con memoria constante y crea otra hoja al alcanzar el límite de filas. |
| **Resaltado SQL con retardo controlado** | Agrupa ediciones rápidas y omite el resaltado cuando el documento supera el umbral de tamaño. |

La exportación de tablas muestra el progreso, permite cancelar, evita exportaciones simultáneas de tablas y utiliza archivos temporales para conservar el destino existente al cancelar. Otros controladores relacionales recurren a la exportación paginada. El rendimiento real depende del servidor, el plan de consulta, la red, el ancho de fila y el formato.

## Cinco idiomas, un mismo entorno

**English · 简体中文 · हिन्दी · Español · Français**

Orvyn ofrece cinco idiomas de interfaz en la misma aplicación. En el primer inicio selecciona el idioma del sistema si es compatible; puedes cambiarlo desde la interfaz y la elección se guarda para los siguientes inicios. Los temas claro y oscuro y las fuentes incluidas **Inter, JetBrains Mono y Noto Sans CJK** completan un entorno pensado para equipos internacionales.

## Fuentes de datos

| Familia | Fuentes compatibles |
| --- | --- |
| **Protocolo MySQL · 8** | MySQL · MariaDB · TiDB · OceanBase · Apache Doris · SelectDB · StarRocks · GoldenDB |
| **Protocolo PostgreSQL · 11** | PostgreSQL · TimescaleDB · Redshift · CockroachDB · openGauss · GaussDB · KWDB · KingbaseES · Vastbase · HighGo · UXDB |
| **Motores relacionales y analíticos · 5** | SQLite · DuckDB · SQL Server · ClickHouse · TDengine (REST) |
| **Documentos, clave-valor y HTTP SQL · 3** | MongoDB · Redis · Cloudflare D1 |
| **Búsqueda · 4** | Elasticsearch · Easysearch · Meilisearch · Manticore Search |
| **ODBC del fabricante · 4** | Oracle · DM (达梦) · XuguDB (虚谷) · Microsoft Access |

Las familias comparten controladores y conservan el tratamiento específico de cada motor. ODBC requiere un controlador compatible instalado en el sistema. Las funciones disponibles dependen del motor, su versión y los permisos.

## Herramientas para el trabajo diario

- **Editor SQL:** múltiples pestañas, resaltado, formato, autocompletado de palabras clave, tablas y columnas con reconocimiento de alias. Ejecuta la sentencia actual, una selección o el script completo, con resultados independientes, duración y filas afectadas. Historial, favoritos, transacciones y cancelación según el motor; MySQL/PostgreSQL admiten cancelación en el servidor cuando este y los permisos lo permiten.
- **Exploración y edición:** árbol de bases de datos, esquemas, tablas, vistas, columnas, índices, claves foráneas, disparadores y rutinas. Paginación, filtros, ordenación y edición por clave primaria, con cambios pendientes y confirmación antes de enviarlos.
- **Diseño de esquemas:** prepara altas, bajas, cambios de nombre y tipos de columnas; administra claves primarias, índices y claves foráneas mediante SQL adaptado al motor. Edita definiciones de disparadores y rutinas, conservando las firmas de sobrecarga de PostgreSQL.
- **Importación y exportación:** CSV, JSON, TSV compatible con Excel y XLSX nativo. Los enteros grandes y decimales se conservan como texto en XLSX. Exportación SQL de estructura o estructura con datos para motores compatibles. Asistente CSV con detección de codificación, vista previa, inserciones por lotes, progreso, cancelación e informe de filas fallidas.
- **Redis:** escaneo por cursor, vista plana o jerárquica, edición de String/Hash/List/Set/Sorted Set, creación, cambio de nombre, borrado por lotes, TTL, valores JSON o sin formato y consola de comandos.
- **MongoDB:** filtros, proyección, ordenación, paginación, agregaciones y creación, modificación o eliminación de documentos individuales.
- **Búsqueda y D1:** Elasticsearch/Easysearch con autocompletado basado en mappings, respuestas originales, paginación profunda `PIT` / `search_after` y exportación JSON de la página actual; exploradores Meilisearch/Manticore; ejecución HTTP SQL en Cloudflare D1 con resultados separados por sentencia.

## Seguridad y control

Credenciales guardadas con cifrado **ChaCha20-Poly1305**, exportación de conexiones sin contraseñas por defecto, cinco modos SSL (`Disable`, `Prefer`, `Require`, `VerifyCa`, `VerifyFull`, según el conector) y modo de conexión de solo lectura. Bloqueos de archivo y guardado atómico protegen la configuración; una clave maestra vacía o ilegible nunca se sustituye silenciosamente.

## Descarga e inicio rápido

Obtén los paquetes en **[GitHub Releases](https://github.com/dbx192/Orvyn/releases/latest)**.

| Plataforma | Paquete del flujo de publicación |
| --- | --- |
| Windows x86_64 | Instalador Inno Setup por usuario, sin elevación de administrador. |
| Linux x86_64 / amd64 | `.deb` con la biblioteca DuckDB incluida. |
| macOS Intel x86_64 | Aplicación `.app` en `.dmg`, con DuckDB. |
| macOS Apple Silicon aarch64 | Aplicación `.app` en `.dmg`, con DuckDB. |

Los scripts Linux y de preparación de DuckDB también contemplan ARM64; el flujo actual publica Linux x86_64.

1. Instala Orvyn y crea una conexión del tipo deseado.
2. Introduce los datos de conexión o la ruta de la base local, prueba y guarda.
3. Explora los objetos, abre una consulta o utiliza el explorador Redis o de búsqueda.

Atajos Windows/Linux: `Ctrl+N` nueva consulta, `Ctrl+Enter` sentencia actual, `Ctrl+Shift+Enter` selección, `Ctrl+Shift+F` formato, `Ctrl+L` búsqueda de objetos y `Ctrl+T` tema. En macOS se utiliza `Cmd`.

## Compilar desde el código fuente

Requisitos: Rust stable (mínimo declarado 1.88; CI usa 1.96.1), Python 3 y herramientas nativas. Para reproducir las versiones publicadas, utiliza la versión de Rust del CI.

- **Windows:** Visual Studio C++ Build Tools y Rust MSVC.
- **Linux:** `build-essential pkg-config unixodbc-dev libgtk-3-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libwayland-dev`.
- **macOS:** herramientas de línea de comandos Xcode y `brew install unixodbc`.

Desde la raíz del repositorio:

```sh
python packaging/prepare-duckdb.py
cargo run -p orvyn --release --locked
```

Comprobaciones del proyecto:

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

DuckDB 1.5.4 se descarga mediante `https://gh-proxy.org/` a `packaging/duckdb-download/local/1.5.4/`. Define `ORVYN_GITHUB_PROXY` como cadena vacía para descargar directamente de GitHub o indica otro prefijo de proxy.

Empaquetado: [Windows](packaging/windows/build-installer.ps1) (Inno Setup 6), [Linux](packaging/linux/package.sh), [macOS](packaging/macos/package.sh). El [flujo de publicación](.github/workflows/ci.yml) se ejecuta con etiquetas `v*`: pruebas nativas y Clippy en tres sistemas, integración con PostgreSQL/MySQL/Redis/Elasticsearch y cuatro compilaciones para publicación.

## Configuración

| Sistema | Directorio |
| --- | --- |
| Windows | `%APPDATA%\Orvyn` |
| Linux | `$XDG_CONFIG_HOME/orvyn` o `~/.config/orvyn` |
| macOS | `~/Library/Application Support/Orvyn` |

`ORVYN_CONFIG_DIR` cambia el directorio. `ORVYN_MASTER_KEY` proporciona la clave de cifrado; si no se define, se crea un `master.key` privado. `RUST_LOG` controla el registro `orvyn.log` del directorio de configuración. **Guarda una copia de la clave junto con las credenciales cifradas: necesitarás la misma clave para recuperarlas.**

## Compatibilidad

- ClickHouse, TDengine, Cloudflare D1 y MongoDB no ofrecen controles de transacción del cliente.
- Cancelar no revierte trabajo confirmado; algunos conectores solo interrumpen la espera del cliente. Los diagnósticos lo distinguen de una cancelación confirmada en el servidor.
- El modo de solo lectura es una protección del cliente; restringe los efectos de funciones del servidor mediante permisos de base de datos.
- Los túneles SSH, la comparación de esquemas/datos, los diagramas ER, la administración de usuarios/permisos y las actualizaciones automáticas aún no están implementados.

## Licencia

Publicado bajo la [licencia MIT](LICENSE).
