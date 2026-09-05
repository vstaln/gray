//! Local proxy with per-upstream adapters.
use std::sync::Arc;
use async_trait::async_trait;
use axum::{body::Body, extract::{Request, State}, http::{HeaderMap, HeaderValue, StatusCode}, response::{IntoResponse, Response}, routing::{any, get}, Json, Router};
use serde_json::{json, Value};
use tower_http::limit::RequestBodyLimitLayer;
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 8645;
const MAX_BODY: usize = 10_000_000;
#[derive(Debug, Clone)]
pub struct UpstreamCredential { pub bearer: String, pub base_url: String }
impl UpstreamCredential { fn auth(&self)->String { format!("Bearer {}", self.bearer) } }
#[async_trait]
pub trait UpstreamAdapter: Send+Sync {
    fn display(&self)->&str;
    fn is_authenticated(&self)->bool;
    async fn get_credential(&self)->anyhow::Result<UpstreamCredential>;
    async fn get_retry_credential(&self,_:&UpstreamCredential,_:u16)->Option<UpstreamCredential>{None}
}
const HOP: &[&str]=&["host","content-length","connection","keep-alive","proxy-authenticate","proxy-authorization","te","trailers","transfer-encoding","upgrade","authorization"];
pub fn filter_headers(m:&HeaderMap)->HeaderMap{
    let mut o=HeaderMap::new();
    for (k,v) in m { if HOP.contains(&k.as_str().to_ascii_lowercase().as_str()){continue} o.insert(k.clone(),v.clone()); }
    o
}
const COMMON: &[&str]=&["/chat/completions","/completions","/embeddings","/models","/responses"];
pub struct OpenRouterAdapter; pub struct XaiAdapter; pub struct CodexAdapter;
#[async_trait]
impl UpstreamAdapter for OpenRouterAdapter{
    fn display(&self)->&str{"OpenRouter"}
    fn is_authenticated(&self)->bool{
        let keys=crate::setup::load_auth_keys();
        if keys.contains_key("openrouter"){return true}
        let p=crate::setup::saved_config_path().unwrap_or_else(|_|std::path::PathBuf::from("/dev/null"));
        crate::setup::load_saved_config_at(&p).api_key.is_some()
    }
    async fn get_credential(&self)->anyhow::Result<UpstreamCredential>{
        let keys=crate::setup::load_auth_keys();
        let p=crate::setup::saved_config_path().unwrap_or_else(|_|std::path::PathBuf::from("/dev/null"));
        let saved=crate::setup::load_saved_config_at(&p);
        let bearer=keys.get("openrouter").cloned().or(saved.api_key.clone()).ok_or_else(||anyhow::anyhow!("Not logged into OpenRouter. Run /connect"))?;
        let base=saved.base_url.clone().unwrap_or_else(||crate::config::DEFAULT_BASE_URL.to_string());
        Ok(UpstreamCredential{bearer,base_url:base.trim_end_matches('/').to_string()})
    }
}
#[async_trait]
impl UpstreamAdapter for XaiAdapter{
    fn display(&self)->&str{"xAI Grok"}
    fn is_authenticated(&self)->bool{crate::oauth::load_auth("xai").is_ok()}
    async fn get_credential(&self)->anyhow::Result<UpstreamCredential>{
        let t=crate::oauth::ensure_access_token("xai").await?;
        Ok(UpstreamCredential{bearer:t,base_url:crate::oauth::XAI_API_BASE.to_string()})
    }
    async fn get_retry_credential(&self,_: &UpstreamCredential,s:u16)->Option<UpstreamCredential>{
        if s!=401{return None}
        let tok=crate::oauth::refresh("xai").await.ok()?;
        Some(UpstreamCredential{bearer:tok.access_token,base_url:crate::oauth::XAI_API_BASE.to_string()})
    }
}
#[async_trait]
impl UpstreamAdapter for CodexAdapter{
    fn display(&self)->&str{"Codex (OpenAI)"}
    fn is_authenticated(&self)->bool{crate::oauth::load_auth("codex").is_ok()}
    async fn get_credential(&self)->anyhow::Result<UpstreamCredential>{
        let t=crate::oauth::ensure_access_token("codex").await?;
        Ok(UpstreamCredential{bearer:t,base_url:crate::oauth::CODEX_API_BASE.to_string()})
    }
    async fn get_retry_credential(&self,_: &UpstreamCredential,s:u16)->Option<UpstreamCredential>{
        if s!=401{return None}
        let tok=crate::oauth::refresh("codex").await.ok()?;
        Some(UpstreamCredential{bearer:tok.access_token,base_url:crate::oauth::CODEX_API_BASE.to_string()})
    }
}
pub fn get_adapter(name:&str)->anyhow::Result<Arc<dyn UpstreamAdapter>>{
    match name.trim().to_ascii_lowercase().as_str(){
        "openrouter"|"open-router"|"or"=>Ok(Arc::new(OpenRouterAdapter)),
        "xai"|"grok"=>Ok(Arc::new(XaiAdapter)),
        "codex"|"openai"|"oai"=>Ok(Arc::new(CodexAdapter)),
        _=>anyhow::bail!("Unknown provider '{name}'. Available: openrouter, xai, codex"),
    }
}
fn adapter_from_base(base:&str)->Arc<dyn UpstreamAdapter>{
    let b=base.to_ascii_lowercase();
    if b.contains("api.x.ai"){Arc::new(XaiAdapter)}else if b.contains("api.openai.com"){Arc::new(CodexAdapter)}else{Arc::new(OpenRouterAdapter)}
}
pub fn default_adapter(c:&crate::config::Config)->Arc<dyn UpstreamAdapter>{adapter_from_base(&c.base_url)}
pub fn router(a:Arc<dyn UpstreamAdapter>)->Router{
    Router::new().route("/health",get(health)).route("/v1/*tail",any(handle_proxy)).with_state(a).layer(RequestBodyLimitLayer::new(MAX_BODY))
}
async fn health(State(a):State<Arc<dyn UpstreamAdapter>>)->Json<Value>{
    Json(json!({"status":"ok","upstream":a.display(),"authenticated":a.is_authenticated()}))
}
async fn handle_proxy(State(a):State<Arc<dyn UpstreamAdapter>>,req:Request)->Response{
    let path=req.uri().path().to_string();
    let tail=path.strip_prefix("/v1").unwrap_or(&path);
    let rel=if tail.is_empty(){"/".to_string()}else if tail.starts_with('/') {tail.to_string()}else{format!("/{tail}")};
    if !COMMON.contains(&rel.as_str()){
        let b=Json(json!({"error":{"message":format!("Path /v1{rel} is not forwarded by this proxy. Allowed: {}",COMMON.join(", ")),"type":"path_not_allowed","code":"path_not_allowed"}}));
        return (StatusCode::NOT_FOUND,b).into_response();
    }
    let cred=match a.get_credential().await{
        Ok(c)=>c,
        Err(e)=>{ let b=Json(json!({"error":{"message":e.to_string(),"type":"upstream_auth_failed","code":"upstream_auth_failed"}})); return (StatusCode::UNAUTHORIZED,b).into_response(); }
    };
    let method=req.method().clone();
    let query=req.uri().query().map(|q|format!("?{q}")).unwrap_or_default();
    let headers_in=req.headers().clone();
    let bytes=match axum::body::to_bytes(req.into_body(),MAX_BODY).await{
        Ok(b)=>b, Err(_)=>{ let b=Json(json!({"error":{"message":"request body too large","type":"proxy_error","code":"proxy_error"}})); return (StatusCode::PAYLOAD_TOO_LARGE,b).into_response(); }
    };
    let mut fwd=filter_headers(&headers_in);
    if let Ok(v)=HeaderValue::from_str(&cred.auth()){ fwd.insert("authorization",v); }
    let url=format!("{}{}{}",cred.base_url.trim_end_matches('/'),rel,query);
    let send = |hdrs:HeaderMap,body:axum::body::Bytes|{
        let url=url.clone(); let meth=method.clone();
        async move{
            let client=reqwest::Client::builder().timeout(std::time::Duration::from_secs(300)).build().map_err(|e|anyhow::anyhow!("{e}"))?;
            let mut rb=client.request(meth,&url);
            for (k,v) in hdrs.iter(){ rb=rb.header(k.as_str(),v.to_str().unwrap_or_default()); }
            if !body.is_empty(){ rb=rb.body(body.to_vec()); }
            let r=rb.send().await.map_err(|e|anyhow::anyhow!("upstream unreachable: {e}"))?;
            Ok::<_,anyhow::Error>(r)
        }
    };
    let upstream=match send(fwd.clone(),bytes.clone()).await{
        Ok(r)=>r, Err(e)=>{ let b=Json(json!({"error":{"message":e.to_string(),"type":"upstream_unreachable","code":"upstream_unreachable"}})); return (StatusCode::BAD_GATEWAY,b).into_response(); }
    };
    let mut final_resp=upstream;
    if final_resp.status()==401{
        if let Some(retry)=a.get_retry_credential(&cred,401).await{
            if retry.bearer!=cred.bearer{
                let mut rh=filter_headers(&headers_in);
                if let Ok(v)=HeaderValue::from_str(&retry.auth()){ rh.insert("authorization",v); }
                if let Ok(r)=send(rh,bytes).await{ final_resp=r; }
            }
        }
    }
    let status=StatusCode::from_u16(final_resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    // filter response headers: strip hop + content-encoding/length
    let mut rh=HeaderMap::new();
    for (k,v) in final_resp.headers(){
        let lk=k.as_str().to_ascii_lowercase();
        if HOP.contains(&lk.as_str()){continue}
        if lk=="content-encoding"||lk=="content-length"{continue}
        rh.insert(k.clone(),v.clone());
    }
    let stream=final_resp.bytes_stream();
    let body=Body::from_stream(stream);
    let mut resp=Response::builder().status(status).body(body).unwrap();
    for (k,v) in rh.iter(){ resp.headers_mut().insert(k.clone(),v.clone()); }
    resp
}
pub async fn run_server(a:Arc<dyn UpstreamAdapter>,host:&str,port:u16)->anyhow::Result<()>{
    let addr=format!("{host}:{port}");
    let l=tokio::net::TcpListener::bind(&addr).await?;
    log::info!("proxy: listening on http://{addr}/v1 -> {}",a.display());
    axum::serve(l,router(a)).await?; Ok(())
}
#[derive(clap::Parser,Debug,Clone)]
pub enum ProxyCmd{
    /// Start proxy — interactive picker if no --provider (like /model), or direct with --provider
    Start{#[arg(long)] provider:Option<String>,#[arg(long,default_value="127.0.0.1")] host:String,#[arg(long,default_value_t=8645)] port:u16},
    /// Show proxy status (auth + running)
    Status,
    /// List available proxy providers
    Providers
}
pub async fn run_cli(cmd:Option<ProxyCmd>,cfg:&crate::config::Config)->anyhow::Result<()>{
    match cmd{
        None|Some(ProxyCmd::Status)=>{
            println!("Gray proxy upstream adapters\n");
            for n in ["openrouter","xai","codex"]{
                let a=get_adapter(n).unwrap();
                if !a.is_authenticated(){ println!("  [{n:8}] {} — not logged in",a.display()); continue; }
                match a.get_credential().await{
                    Ok(_)=>println!("  [{n:8}] {} — ready",a.display()),
                    Err(e)=>println!("  [{n:8}] {} — credentials need attention ({e})",a.display()),
                }
            }
            println!("\nStart the proxy with: gray proxy start [--provider <name>]"); Ok(())
        }
        Some(ProxyCmd::Providers)=>{
            println!("Available proxy upstream providers:");
            for n in ["openrouter","xai","codex"]{ let a=get_adapter(n).unwrap(); println!("  {n:10} — {}",a.display()); }
            Ok(())
        }
        Some(ProxyCmd::Start{provider,host,port})=>{
            let a=if let Some(p)=provider{get_adapter(&p)?}else{adapter_from_base(&cfg.base_url)};
            if !a.is_authenticated(){ anyhow::bail!("Not logged into {}. Run /connect first.",a.display()); }
            println!("Listening on http://{host}:{port}/v1 -> {} ✓",a.display());
            run_server(a,&host,port).await
        }
    }
}
#[cfg(test)]
mod tests{
    use super::*; use axum::http::{HeaderMap,HeaderValue}; use tower::ServiceExt;
    #[test] fn filter_strips_hop_by_hop(){
        let mut m=HeaderMap::new();
        m.insert("authorization",HeaderValue::from_static("Bearer x"));
        m.insert("content-type",HeaderValue::from_static("application/json"));
        let f=filter_headers(&m); assert!(!f.contains_key("authorization")); assert!(f.contains_key("content-type"));
    }
    #[tokio::test] async fn health_returns_ok(){
        let a:Arc<dyn UpstreamAdapter>=Arc::new(OpenRouterAdapter);
        let app=router(a);
        let req=Request::builder().uri("/health").body(Body::empty()).unwrap();
        let resp=app.oneshot(req).await.unwrap(); assert_eq!(resp.status(),200);
        let body=axum::body::to_bytes(resp.into_body(),1024).await.unwrap();
        let v:Value=serde_json::from_slice(&body).unwrap(); assert_eq!(v["status"],"ok");
    }
}
