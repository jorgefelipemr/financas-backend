use axum::{
    extract::{Path, Query, State},
    http::{Request, StatusCode, Method},
    middleware::{self, Next},
    response::Response,
    routing::{delete, get, post},
    Json, Router,
};
use dotenvy::dotenv;
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Pool, Postgres, Row};
use std::collections::HashMap;
use std::env;
use tower_http::cors::{Any, CorsLayer};

// --- Estruturas de Dados ---
#[derive(Deserialize)]
struct NovoGasto {
    descricao: String,
    valor: f64,
    tipo: String,
    categoria_id: i32,
    responsavel: String,
    data: Option<String>,
}

#[derive(Serialize, sqlx::FromRow)]
struct Categoria { id: i32, nome: String }

#[derive(Serialize)]
struct Gasto {
    id: i32,
    descricao: String,
    valor: String,
    responsavel: String,
    data: String,
}

#[derive(Serialize)]
struct Estatisticas { categoria: String, total: f64 }

#[derive(Serialize)]
struct ResumoFinanceiro {
    saldo: f64,
    receitas: f64,
    despesas: f64,
    stats: Vec<Estatisticas>,
}

#[derive(Serialize, Deserialize)]
struct Objetivo {
    id: Option<i32>,
    nome: String,
    valor_total: f64,
    valor_guardado: f64,
    data_limite: String,
}

// --- Middleware de Segurança ---
async fn validador_seguranca(req: Request<axum::body::Body>, next: Next) -> Result<Response, StatusCode> {
    // O CORS agora lida com as requisições OPTIONS automaticamente antes de chegar aqui
    let auth_header = req.headers().get("x-api-key").and_then(|h| h.to_str().ok());
    let chave_mestra = "JORGE_E_LETICIA_2026"; 

    if let Some(chave) = auth_header {
        if chave == chave_mestra {
            return Ok(next.run(req).await);
        }
    }
    Err(StatusCode::UNAUTHORIZED)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();
    
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL não configurada");
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    println!("✅ Backend Seguro e Online - Jorge & Leticia");

    // Configuração de CORS: ESSENCIAL para GitHub Pages e Preflight (OPTIONS)
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
        .allow_headers(Any);

    let app = Router::new()
        .route("/lancar", post(handler_lancar))
        .route("/listar", get(handler_listar))
        .route("/transacao/:id", delete(handler_deletar_transacao))
        .route("/resumo", get(handler_resumo))
        .route("/categorias", get(handler_categorias))
        .route("/categorias", post(handler_criar_categoria))
        .route("/objetivos", get(handler_listar_objetivos))
        .route("/objetivos", post(handler_criar_objetivo))
        .route("/objetivos/:id", delete(handler_deletar_objetivo))
        .route("/objetivos/:id", post(handler_editar_objetivo))
        .route("/objetivos/:id/aportar", post(handler_aportar))
        // ATENÇÃO À ORDEM DOS LAYERS:
        // O layer adicionado por último é o PRIMEIRO a processar a requisição.
        .layer(middleware::from_fn(validador_seguranca)) 
        .layer(cors) // O CORS DEVE VIR POR FORA DA SEGURANÇA
        .with_state(pool);

    let port = env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

// --- Handlers ---

async fn handler_resumo(State(pool): State<Pool<Postgres>>, Query(params): Query<HashMap<String, String>>) -> Result<Json<ResumoFinanceiro>, String> {
    let mes_p = params.get("mes").cloned().unwrap_or_else(|| "4".to_string());
    let condicao = if mes_p == "todos" { "TRUE".to_string() } else { format!("EXTRACT(MONTH FROM data_criacao) = {}", mes_p) };

    let row_balanco = sqlx::query(&format!("SELECT COALESCE(SUM(CASE WHEN tipo = 'Receita' THEN valor ELSE 0 END), 0) as receitas, COALESCE(SUM(CASE WHEN tipo = 'Despesa' THEN valor ELSE 0 END), 0) as despesas FROM transacoes WHERE {}", condicao))
        .fetch_one(&pool).await.map_err(|e| e.to_string())?;

    let rec: sqlx::types::BigDecimal = row_balanco.get("receitas");
    let des: sqlx::types::BigDecimal = row_balanco.get("despesas");

    let stats_rows = sqlx::query(&format!("SELECT c.nome as categoria, SUM(t.valor) as total FROM transacoes t JOIN categorias c ON t.categoria_id = c.id WHERE t.tipo = 'Despesa' AND {} GROUP BY c.nome", condicao))
        .fetch_all(&pool).await.map_err(|e| e.to_string())?;

    let stats = stats_rows.iter().map(|r| {
        let t: sqlx::types::BigDecimal = r.get("total");
        Estatisticas { categoria: r.get("categoria"), total: t.to_string().parse().unwrap_or(0.0) }
    }).collect();

    Ok(Json(ResumoFinanceiro { 
        receitas: rec.to_string().parse().unwrap_or(0.0), 
        despesas: des.to_string().parse().unwrap_or(0.0), 
        saldo: (rec - des).to_string().parse().unwrap_or(0.0), 
        stats 
    }))
}

async fn handler_lancar(State(pool): State<Pool<Postgres>>, Json(p): Json<NovoGasto>) -> Result<Json<String>, String> {
    let data = p.data.unwrap_or_else(|| "NOW()".to_string());
    sqlx::query("INSERT INTO transacoes (descricao, valor, tipo, categoria_id, responsavel, data_criacao) VALUES ($1, $2, $3, $4, $5, $6::date)")
        .bind(&p.descricao).bind(p.valor).bind(&p.tipo).bind(p.categoria_id).bind(&p.responsavel).bind(data)
        .execute(&pool).await.map_err(|e| e.to_string())?;
    Ok(Json("Ok".to_string()))
}

async fn handler_listar(State(pool): State<Pool<Postgres>>) -> Result<Json<Vec<Gasto>>, String> {
    let rows = sqlx::query("SELECT id, descricao, valor, responsavel, TO_CHAR(data_criacao, 'DD/MM') as data_fmt FROM transacoes ORDER BY data_criacao DESC, id DESC LIMIT 20")
        .fetch_all(&pool).await.map_err(|e| e.to_string())?;
    Ok(Json(rows.iter().map(|r| {
        let v: sqlx::types::BigDecimal = r.get("valor");
        Gasto { id: r.get("id"), descricao: r.get("descricao"), valor: format!("{:.2}", v), responsavel: r.get("responsavel"), data: r.get("data_fmt") }
    }).collect()))
}

async fn handler_deletar_transacao(State(pool): State<Pool<Postgres>>, Path(id): Path<i32>) -> Result<Json<String>, String> {
    sqlx::query("DELETE FROM transacoes WHERE id = $1").bind(id).execute(&pool).await.map_err(|e| e.to_string())?;
    Ok(Json("Ok".to_string()))
}

async fn handler_listar_objetivos(State(pool): State<Pool<Postgres>>) -> Result<Json<Vec<Objetivo>>, String> {
    let rows = sqlx::query("SELECT id, nome, valor_total, valor_guardado, data_limite::text FROM objetivos ORDER BY id DESC")
        .fetch_all(&pool).await.map_err(|e| e.to_string())?;
    Ok(Json(rows.iter().map(|r| {
        let vt: sqlx::types::BigDecimal = r.get("valor_total");
        let vg: sqlx::types::BigDecimal = r.get("valor_guardado");
        Objetivo { id: Some(r.get("id")), nome: r.get("nome"), valor_total: vt.to_string().parse().unwrap_or(0.0), valor_guardado: vg.to_string().parse().unwrap_or(0.0), data_limite: r.get("data_limite") }
    }).collect()))
}

async fn handler_criar_objetivo(State(pool): State<Pool<Postgres>>, Json(p): Json<Objetivo>) -> Result<Json<String>, String> {
    sqlx::query("INSERT INTO objetivos (nome, valor_total, valor_guardado, data_limite) VALUES ($1, $2, $3, $4::date)")
        .bind(&p.nome).bind(p.valor_total).bind(p.valor_guardado).bind(&p.data_limite)
        .execute(&pool).await.map_err(|e| e.to_string())?;
    Ok(Json("Ok".to_string()))
}

async fn handler_deletar_objetivo(State(pool): State<Pool<Postgres>>, Path(id): Path<i32>) -> Result<Json<String>, String> {
    sqlx::query("DELETE FROM objetivos WHERE id = $1").bind(id).execute(&pool).await.map_err(|e| e.to_string())?;
    Ok(Json("Removido".to_string()))
}

async fn handler_editar_objetivo(State(pool): State<Pool<Postgres>>, Path(id): Path<i32>, Json(p): Json<serde_json::Value>) -> Result<Json<String>, String> {
    let nome = p["nome"].as_str().ok_or("Nome inválido")?;
    let valor_total = p["valor_total"].as_f64().ok_or("Valor inválido")?;
    let data_limite = p["data_limite"].as_str().ok_or("Data inválida")?;
    sqlx::query("UPDATE objetivos SET nome = $1, valor_total = $2, data_limite = $3::date WHERE id = $4")
        .bind(nome).bind(valor_total).bind(data_limite).bind(id)
        .execute(&pool).await.map_err(|e| e.to_string())?;
    Ok(Json("Atualizado".to_string()))
}

async fn handler_aportar(State(pool): State<Pool<Postgres>>, Path(id): Path<i32>, Json(p): Json<serde_json::Value>) -> Result<Json<String>, String> {
    let valor = p["valor"].as_f64().unwrap_or(0.0);
    sqlx::query("UPDATE objetivos SET valor_guardado = valor_guardado + $1 WHERE id = $2").bind(valor).bind(id).execute(&pool).await.map_err(|e| e.to_string())?;
    
    let row_nome = sqlx::query("SELECT nome FROM objetivos WHERE id = $1").bind(id).fetch_one(&pool).await.map_err(|e| e.to_string())?;
    let nome: String = row_nome.get("nome");

    sqlx::query("INSERT INTO transacoes (descricao, valor, tipo, categoria_id, responsavel, data_criacao) VALUES ($1, $2, 'Despesa', (SELECT id FROM categorias LIMIT 1), 'Sistema', NOW())")
        .bind(format!("Aporte: {}", nome)).bind(valor).execute(&pool).await.map_err(|e| e.to_string())?;
    Ok(Json("Ok".to_string()))
}

async fn handler_categorias(State(pool): State<Pool<Postgres>>) -> Result<Json<Vec<Categoria>>, String> {
    let rows = sqlx::query_as::<_, Categoria>("SELECT id, nome FROM categorias ORDER BY nome").fetch_all(&pool).await.map_err(|e| e.to_string())?;
    Ok(Json(rows))
}

async fn handler_criar_categoria(State(pool): State<Pool<Postgres>>, Json(p): Json<serde_json::Value>) -> Result<Json<String>, String> {
    let n = p["nome"].as_str().ok_or("Invalido")?;
    sqlx::query("INSERT INTO categorias (nome) VALUES ($1)").bind(n).execute(&pool).await.map_err(|e| e.to_string())?;
    Ok(Json("Ok".to_string()))
}