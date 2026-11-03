use std::{
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path as AxumPath, Query, State},
    http::{
        HeaderMap, Request, StatusCode,
        header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE},
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, postgres::PgPoolOptions};
use subtle::ConstantTimeEq;
use tokio::{
    fs::{self, File},
    io::AsyncWriteExt,
};
use uuid::Uuid;

const API_KEY_HEADER: &str = "hap-api-key";
const MAX_UPLOAD_SIZE: usize = 20 * 1024 * 1024;

#[derive(Clone)]
struct AppState {
    pool: PgPool,
    api_key: String,
    images_dir: PathBuf,
}

#[derive(Serialize, sqlx::FromRow)]
struct UploadResponse {
    id: Uuid,
    file_name: String,
    original_name: String,
}

#[derive(Deserialize)]
struct PaginationParams {
    #[serde(default = "default_page")]
    page: i64,
    #[serde(default = "default_limit")]
    limit: i64,
}

#[derive(Serialize)]
struct ImagesResponse {
    total_records: i64,
    page: i64,
    limit: i64,
    data: Vec<UploadResponse>,
}

#[derive(Serialize)]
struct ImagePathResponse {
    path: String,
}

#[derive(Debug)]
enum AppError {
    BadRequest(String),
    NotFound,
    Unauthorized,
    Internal,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            Self::NotFound => (StatusCode::NOT_FOUND, "Imagen no encontrada".to_owned()),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "API key inválida".to_owned()),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Error interno del servidor".to_owned(),
            ),
        };

        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    let database_url = required_env("HAP_FILES_DATABASE_URL")?;
    let api_key = required_env("HAP_API_KEY")?;
    let images_dir = PathBuf::from(env::var("IMAGES_DIR").unwrap_or_else(|_| "images".into()));
    let bind_address = env::var("BIND_ADDRESS").unwrap_or_else(|_| "0.0.0.0:3001".into());

    fs::create_dir_all(&images_dir).await?;

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;
    sqlx::migrate!().run(&pool).await?;

    let state = AppState {
        pool,
        api_key,
        images_dir,
    };

    let protected_routes = Router::new()
        .route("/images", get(get_images).post(upload_image))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_api_key,
        ));

    let app = Router::new()
        .merge(protected_routes)
        .route("/images/{file_name}", get(get_image))
        .route("/public/images/{file_name}", get(serve_image))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_SIZE))
        .with_state(state);

    let address: SocketAddr = bind_address.parse()?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("Servidor escuchando en http://{address}");
    axum::serve(listener, app).await?;

    Ok(())
}

async fn require_api_key(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, AppError> {
    let supplied_key = request
        .headers()
        .get(API_KEY_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or(AppError::Unauthorized)?;

    let expected = state.api_key.as_bytes();
    let supplied = supplied_key.as_bytes();
    let is_valid = expected.len() == supplied.len() && bool::from(expected.ct_eq(supplied));

    if !is_valid {
        return Err(AppError::Unauthorized);
    }

    Ok(next.run(request).await)
}

async fn upload_image(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, AppError> {
    // Evita aceptar cuerpos multipart sin un límite conocido aunque cambie la capa global.
    if headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_UPLOAD_SIZE)
    {
        return Err(AppError::BadRequest(
            "El archivo es demasiado grande".into(),
        ));
    }

    let mut uploaded = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| AppError::BadRequest("Multipart inválido".into()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        if uploaded.is_some() {
            return Err(AppError::BadRequest(
                "Solo se permite un archivo por solicitud".into(),
            ));
        }

        uploaded = Some(save_image(&state, field).await?);
    }

    let response = uploaded.ok_or_else(|| AppError::BadRequest("Falta el campo 'file'".into()))?;

    Ok((StatusCode::CREATED, Json(response)))
}

async fn get_images(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<ImagesResponse>, AppError> {
    if params.page < 1 {
        return Err(AppError::BadRequest(
            "El parámetro 'page' debe ser mayor o igual a 1".into(),
        ));
    }
    if !(1..=100).contains(&params.limit) {
        return Err(AppError::BadRequest(
            "El parámetro 'limit' debe estar entre 1 y 100".into(),
        ));
    }

    let offset = (params.page - 1)
        .checked_mul(params.limit)
        .ok_or_else(|| AppError::BadRequest("Los parámetros de paginación son inválidos".into()))?;

    let total_records: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM images")
        .fetch_one(&state.pool)
        .await
        .map_err(|_| AppError::Internal)?;

    let images = sqlx::query_as::<_, UploadResponse>(
        "SELECT id, file_name, original_name
         FROM images
         ORDER BY created_at DESC
         LIMIT $1 OFFSET $2",
    )
    .bind(params.limit)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    .map_err(|_| AppError::Internal)?;

    let response = ImagesResponse {
        total_records,
        page: params.page,
        limit: params.limit,
        data: images,
    };

    Ok(Json(response))
}

fn default_page() -> i64 {
    1
}

fn default_limit() -> i64 {
    20
}

async fn get_image(
    State(state): State<AppState>,
    AxumPath(file_name): AxumPath<String>,
) -> Result<Json<ImagePathResponse>, AppError> {
    resolve_image_path(&state, &file_name).await?;

    Ok(Json(ImagePathResponse {
        path: format!("/public/images/{file_name}"),
    }))
}

async fn serve_image(
    State(state): State<AppState>,
    AxumPath(file_name): AxumPath<String>,
) -> Result<Response, AppError> {
    let image_path = resolve_image_path(&state, &file_name).await?;
    let bytes = fs::read(image_path).await.map_err(|_| AppError::Internal)?;

    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "image/avif")
        .header(
            CONTENT_DISPOSITION,
            format!("inline; filename=\"{file_name}\""),
        )
        .header(CACHE_CONTROL, "public, max-age=86400")
        .body(Body::from(bytes))
        .map_err(|_| AppError::Internal)
}

async fn resolve_image_path(state: &AppState, file_name: &str) -> Result<PathBuf, AppError> {
    let id = file_name
        .strip_suffix(".avif")
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|id| file_name == format!("{id}.avif"))
        .ok_or(AppError::NotFound)?;

    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM images WHERE id = $1)")
        .bind(id)
        .fetch_one(&state.pool)
        .await
        .map_err(|_| AppError::Internal)?;

    if !exists {
        return Err(AppError::NotFound);
    }

    let image_path = state.images_dir.join(&file_name);
    fs::metadata(&image_path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            AppError::NotFound
        } else {
            AppError::Internal
        }
    })?;

    Ok(image_path) 
}

async fn save_image(
    state: &AppState,
    mut field: axum::extract::multipart::Field<'_>,
) -> Result<UploadResponse, AppError> {
    let original_name = field
        .file_name()
        .and_then(|name| Path::new(name).file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| AppError::BadRequest("El archivo no tiene un nombre válido".into()))?
        .to_owned();

    let has_avif_extension = Path::new(&original_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("avif"));
    if !has_avif_extension {
        return Err(AppError::BadRequest(
            "El archivo debe tener extensión .avif".into(),
        ));
    }

    if field.content_type() != Some("image/avif") {
        return Err(AppError::BadRequest(
            "El Content-Type del archivo debe ser image/avif".into(),
        ));
    }

    let id = Uuid::new_v4();
    let file_name = format!("{id}.avif");
    let temporary_path = state.images_dir.join(format!(".{id}.upload"));
    let final_path = state.images_dir.join(&file_name);
    let mut file = File::create(&temporary_path)
        .await
        .map_err(|_| AppError::Internal)?;
    let mut signature = Vec::with_capacity(64);

    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|_| AppError::BadRequest("No se pudo leer el archivo".into()))?
    {
        if signature.len() < 64 {
            let remaining = 64 - signature.len();
            signature.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        }
        file.write_all(&chunk)
            .await
            .map_err(|_| AppError::Internal)?;
    }

    file.flush().await.map_err(|_| AppError::Internal)?;
    drop(file);

    if !is_avif(&signature) {
        let _ = fs::remove_file(&temporary_path).await;
        return Err(AppError::BadRequest(
            "El contenido no corresponde a una imagen AVIF".into(),
        ));
    }

    fs::rename(&temporary_path, &final_path)
        .await
        .map_err(|_| AppError::Internal)?;

    let insert_result =
        sqlx::query("INSERT INTO images (id, file_name, original_name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(&file_name)
            .bind(&original_name)
            .execute(&state.pool)
            .await;

    if insert_result.is_err() {
        let _ = fs::remove_file(&final_path).await;
        return Err(AppError::Internal);
    }

    Ok(UploadResponse {
        id,
        file_name,
        original_name,
    })
}

fn is_avif(bytes: &[u8]) -> bool {
    bytes.len() >= 16
        && &bytes[4..8] == b"ftyp"
        && bytes[8..]
            .chunks_exact(4)
            .any(|brand| brand == b"avif" || brand == b"avis")
}

fn required_env(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    env::var(name)
        .map_err(|_| format!("Falta la variable de entorno {name}").into())
        .and_then(|value: String| {
            if value.is_empty() {
                Err(format!("La variable de entorno {name} no puede estar vacía").into())
            } else {
                Ok(value)
            }
        })
}
