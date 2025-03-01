//!  (A)utomatic (P)ersisted (Q)ueries cache.
//!
//!  For more information on APQ see:
//!  <https://www.apollographql.com/docs/apollo-server/performance/apq/>

// This entire file is license key functionality

use std::borrow::Cow;
use std::ops::ControlFlow;

use askama::Template;
use http::header::CONTENT_TYPE;
use http::HeaderMap;
use http::HeaderValue;
use http::Method;
use hyper::Body;
use mediatype::names::HTML;
use mediatype::names::TEXT;
use mediatype::MediaType;
use mediatype::MediaTypeList;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use tower::BoxError;
use tower::Layer;
use tower::Service;

use crate::layers::sync_checkpoint::CheckpointService;
use crate::services::router;
use crate::Configuration;

/// [`Layer`] That serves Static pages such as Homepage and Sandbox.
#[derive(Clone)]
pub(crate) struct StaticPageLayer {
    static_page: Option<String>,
}

impl StaticPageLayer {
    pub(crate) fn new(configuration: &Configuration) -> Self {
        let static_page = if configuration.sandbox.enabled {
            Some(sandbox_page_content())
        } else if configuration.homepage.enabled {
            Some(home_page_content())
        } else {
            None
        };

        Self { static_page }
    }
}

impl<S> Layer<S> for StaticPageLayer
where
    S: Service<router::Request, Response = router::Response, Error = BoxError> + Send + 'static,
    <S as Service<router::Request>>::Future: Send + 'static,
{
    type Service = CheckpointService<S, router::Request>;

    fn layer(&self, service: S) -> Self::Service {
        if let Some(page) = self.static_page.as_ref() {
            let page = page.clone();
            let cow = Cow::from(page);

            CheckpointService::new(
                move |req| {
                    let res = if req.router_request.method() == Method::GET
                        && prefers_html(req.router_request.headers())
                    {
                        let response = http::Response::builder()
                            .header(
                                CONTENT_TYPE,
                                HeaderValue::from_static(mime::TEXT_HTML_UTF_8.as_ref()),
                            )
                            .body(Body::from(cow.clone()))
                            .unwrap();
                        ControlFlow::Break(router::Response {
                            response,
                            context: req.context,
                        })
                    } else {
                        ControlFlow::Continue(req)
                    };

                    Ok(res)
                },
                service,
            )
        } else {
            CheckpointService::new(move |req| Ok(ControlFlow::Continue(req)), service)
        }
    }
}

fn prefers_html(headers: &HeaderMap) -> bool {
    let text_html = MediaType::new(TEXT, HTML);

    headers.get_all(&http::header::ACCEPT).iter().any(|value| {
        value
            .to_str()
            .map(|accept_str| {
                let mut list = MediaTypeList::new(accept_str);

                list.any(|mime| mime.as_ref() == Ok(&text_html))
            })
            .unwrap_or(false)
    })
}

/// Configuration options pertaining to the sandbox page.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Sandbox {
    #[serde(default = "default_sandbox")]
    pub(crate) enabled: bool,
}

fn default_sandbox() -> bool {
    false
}

#[buildstructor::buildstructor]
impl Sandbox {
    #[builder]
    pub(crate) fn new(enabled: Option<bool>) -> Self {
        Self {
            enabled: enabled.unwrap_or_else(default_sandbox),
        }
    }
}

impl Default for Sandbox {
    fn default() -> Self {
        Self::builder().build()
    }
}

#[derive(Template)]
#[template(path = "sandbox_index.html")]
struct SandboxTemplate {}

pub(crate) fn sandbox_page_content() -> String {
    let template = SandboxTemplate {};
    template.render().expect("cannot fail")
}

#[derive(Template)]
#[template(path = "homepage_index.html")]
struct HomepageTemplate {}

pub(crate) fn home_page_content() -> String {
    let template = HomepageTemplate {};
    template.render().expect("cannot fail")
}
