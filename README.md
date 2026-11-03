# hap-files

Servicio HTTP en Rust para almacenar imágenes AVIF y registrar sus nombres en
PostgreSQL.

## Configuración

1. Copia y ajusta las variables de entorno:

   ```bash
   cp .env.template .env
   ```

2. Levanta PostgreSQL:

   ```bash
   docker compose up -d
   ```

   Puedes comprobar su estado con:

   ```bash
   docker compose ps
   ```

3. Inicia el servicio. La migración de PostgreSQL y el directorio de imágenes se crean
   automáticamente:

   ```bash
   cargo run
   ```

La configuración predeterminada conecta el servicio ejecutado en la máquina
local con PostgreSQL expuesto por Docker en `localhost:5432`. Si cambias las
credenciales o el puerto de PostgreSQL en `.env`,
`HAP_FILES_DATABASE_URL` se construye automáticamente a partir de las variables
`HAP_FILES_DB_*`.

## Subir una imagen

El endpoint espera `multipart/form-data`, con el archivo en el campo `file` y
la API key en la cabecera `x-api-key`:

```bash
curl --fail-with-body \
  -X POST http://localhost:3000/images \
  -H 'x-api-key: cambia-esta-clave' \
  -F 'file=@foto.avif;type=image/avif'
```

La extensión, el tipo MIME y la firma básica del contenedor AVIF son
validados. El límite por solicitud es de 20 MiB. El archivo se guarda bajo un
nombre UUID para evitar colisiones, mientras que PostgreSQL conserva tanto ese
nombre como el nombre original.

## Listar imágenes

El endpoint devuelve las imágenes registradas, ordenadas desde la más reciente.
Acepta los parámetros `page` (predeterminado: `1`) y `limit` (predeterminado:
`20`, máximo: `100`):

```bash
curl --fail-with-body \
  'http://localhost:3000/images?page=1&limit=20' \
  -H 'hap-api-key: cambia-esta-clave'
```

La respuesta contiene `total_records`, `page`, `limit` y `data`.

## Obtener la ruta de una imagen

Usa el `file_name` devuelto por el endpoint de subida como parámetro del path:

```bash
curl --fail-with-body \
  http://localhost:3000/images/bbd3fa9e-3374-4233-9469-1023dda8f468.avif
```

Este endpoint es público, comprueba que la imagen exista y responde con su
ruta web:

```json
{
  "path": "/public/images/bbd3fa9e-3374-4233-9469-1023dda8f468.avif"
}
```

La ruta devuelta también es pública y sirve directamente la imagen AVIF:

```bash
curl --fail-with-body \
  http://localhost:3000/public/images/bbd3fa9e-3374-4233-9469-1023dda8f468.avif \
  --output imagen.avif
```

La URL pública es independiente de `IMAGES_DIR`, que continúa siendo la
ubicación física privada de los archivos.
