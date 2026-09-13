//! FORK: `POST /resource` -- the still image every HomeKit camera has to serve.
//!
//! This route did not exist in this crate at all, and its absence is what kept a camera showing
//! **"No Response"** in Apple Home long after pairing, `/accessories` and the stream-configuration
//! characteristics were all correct. iOS reads the accessory database, sees an IP Camera, and
//! immediately asks for a snapshot to draw on the tile; a 404 leaves it with no image and nothing
//! to display.
//!
//! The snapshot is supplied by the consumer through [`IpServer::set_snapshot_provider`], because
//! only the consumer knows where frames come from. When none is registered the route reports
//! `ResourceDoesNotExist`, which at least says so honestly rather than 404-ing.

use hyper::{
    body::Buf,
    header::{CONTENT_LENGTH, CONTENT_TYPE},
    Body,
    Response,
    StatusCode,
};
use serde::Deserialize;

use crate::Result;

/// Produces a JPEG of roughly the requested size.
///
/// The dimensions are what iOS asked for, and are a hint: controllers scale whatever they get, and
/// real cameras routinely return a single fixed-size still. Returning an error surfaces as
/// `ResourceDoesNotExist` rather than taking the connection down.
pub type SnapshotFn = Box<dyn Fn(u16, u16) -> Result<Vec<u8>> + Send + Sync>;

/// The body iOS sends, e.g.
/// `{"aid":1,"image-width":640,"image-height":360,"resource-type":"image"}`.
#[derive(Debug, Deserialize)]
pub struct SnapshotRequest {
    #[serde(rename = "resource-type")]
    pub resource_type: Option<String>,
    #[serde(rename = "image-width")]
    pub image_width: Option<u16>,
    #[serde(rename = "image-height")]
    pub image_height: Option<u16>,
}

impl SnapshotRequest {
    /// Falls back to 1920x1080 rather than refusing, since the size is only a hint.
    pub fn dimensions(&self) -> (u16, u16) {
        (self.image_width.unwrap_or(1920), self.image_height.unwrap_or(1080))
    }
}

/// Parses the request body, tolerating a body that is absent or unparseable.
pub async fn parse(body: Body) -> SnapshotRequest {
    match hyper::body::aggregate(body).await {
        Ok(aggregated) => serde_json::from_slice(aggregated.chunk()).unwrap_or(SnapshotRequest {
            resource_type: None,
            image_width: None,
            image_height: None,
        }),
        Err(_) => SnapshotRequest {
            resource_type: None,
            image_width: None,
            image_height: None,
        },
    }
}

/// Wraps JPEG bytes in the response iOS expects.
pub fn image_response(image: Vec<u8>) -> Result<Response<Body>> {
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "image/jpeg")
        .header(CONTENT_LENGTH, image.len())
        .body(Body::from(image))?)
}
