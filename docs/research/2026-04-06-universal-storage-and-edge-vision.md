# Universal Storage & Edge Vision: The "Billion Dollar Pitch"

**Date:** 2026-04-06
**Status:** Strategic Vision & Technical Proposal
**Author:** Gemini (Platform Architect)
**Ref:** `docs/specs/2026-04-06-konf-backend-spec-v2.md`, `docs/research/2026-04-06-rust-ecosystem-survey.md`

---

## 1. Executive Summary

Current AI agent frameworks (LangChain, AutoGen, CrewAI) suffer from three fatal flaws: they are **too heavy** (Python-dependent), **too leaky** (data privacy depends on prompts), and **too centralized** (require cloud infra). 

Konf solves this by building a **Universal Agent Layer** in pure Rust. By combining a **Capability Lattice** with a **Pluggable Storage Layer** (Postgres for Cloud, SQLite + Native Vector for Edge), Konf enables agents to run with high performance and total privacy on anything from a $50,000 server to a $500 smartphone—using the exact same configuration and security rules.

---

## 2. The Vision: "Single Core, Infinite Hosts"

The vision is a single Rust-based engine that "owns" the memory, security, and execution of an agent. 

- **Homelab/Cloud:** Runs as a high-throughput Axum server backed by PostgreSQL/pgvector.
- **Edge/Phone:** Runs as a native binary (via UniFFI) backed by local SQLite and a Rust-native vector index.

**Why this is a "Billion Dollar Pitch":**
Enterprises are desperate for AI but terrified of data leaks. A platform that offers **structural security** (cannot be bypassed by prompt injection) and **local-first execution** (data never leaves the device) allows AI to be deployed in highly regulated industries (Healthcare, Law, Defense) where cloud-only solutions are currently banned.

---

## 3. The Technical Shift: From "Postgres-First" to "Trait-First"

To achieve this, the architecture must move from hardcoded SQL to abstract traits.

### 3.1 The Storage Abstraction
We replace direct `sqlx::PgPool` calls with a `StorageBackend` trait.

```rust
#[async_trait]
pub trait StorageProvider: Send + Sync {
    // Metadata operations (SQL)
    async fn add_node(&self, node: Node) -> Result<Uuid, StorageError>;
    async fn get_node(&self, id: Uuid) -> Result<Option<Node>, StorageError>;
    
    // Vector operations
    async fn vector_search(&self, query: Vec<f32>, limit: usize) -> Result<Vec<SearchResult>, StorageError>;
}
```

### 3.2 The Vector Dilemma: Remote vs. Local
Vector search (similarity search) is the "brain" of the agent. There are two ways to do it:

1.  **Remote (pgvector):** Good for scale. You send a query to Postgres; it returns results. 
    - *Constraint:* High latency (50-200ms) due to network and SQL overhead.
2.  **Local (Rust-Native HNSW):** Mandatory for Edge/Phone. You search an in-memory or memory-mapped index.
    - *Constraint:* Requires local persistence and sync logic.

**Implementation Strategy: Hybrid Vector Store**
We will implement a `VectorStore` trait with two providers:
- **`PgVectorStore`**: Generates SQL for `pgvector`.
- **`LocalVectorStore`**: Wraps a Rust-native library like **`hnsw_rs`** or **`lance`**.

---

## 4. Why Rust-Native Vector Indices?

For edge and mobile use cases, a Rust-native index (like HNSW) is superior to a remote database.

### 4.1 Performance (The "Feel" of Intelligence)
- **Latency:** Local HNSW search takes **~1ms-5ms**, compared to **50ms-200ms** for a remote database. For an agent, this is the difference between "laggy tool" and "instant reasoning."
- **SIMD Optimization:** Rust can utilize native CPU instructions (AVX-512, NEON) to perform vector math at hardware speeds, making "weak" mobile chips perform like servers.

### 4.2 Security & Privacy
In a "Data Security First" model, the **Capability Lattice** in the Rust core ensures that:
- The LLM never sees the namespace or raw vector data.
- Search queries are intercepted and scoped *before* they hit the index.
- If the agent is offline (Edge), the data **physically cannot** be leaked to the cloud.

---

## 5. Implementation Roadmap

### Phase B.5: Storage Abstraction (The Foundation)
- Refactor `smrti` and `konf-runtime` to use a pluggable `StorageProvider` trait.
- Implement the `PostgresProvider` (parity with current spec).
- **Risk:** High engineering effort in `sqlx` trait abstraction.

### Phase E: Edge & Mobile (The Future)
- Implement `SqliteProvider` for metadata.
- Implement `LocalVectorStore` using **`hnsw_rs`** (for speed) or **`lance`** (for disk-based storage).
- Use **UniFFI** to generate Swift/Kotlin bindings for native mobile apps.

---

## 6. Industry Context & Competition

| Feature | LangChain / Python | OpenAI Custom GPTs | **Konf (Rust Core)** |
| :--- | :--- | :--- | :--- |
| **Execution** | Heavy, Slow | Centralized Cloud | **Single Binary, Fast** |
| **Security** | Prompt-based (Soft) | Black Box | **Lattice-based (Hard)** |
| **Privacy** | Cloud-dependent | Owned by OpenAI | **Local-First / Private** |
| **Portability** | Requires Server | No | **Server, Phone, Edge** |

---

## 7. Risks & Trade-offs

- **Engineering Effort:** Writing a robust, concurrent, disk-persisted vector index wrapper in Rust is significantly harder than calling a Postgres API.
- **Sync Complexity:** Syncing a local SQLite/HNSW graph from a phone back to a central Postgres/pgvector homelab requires careful "Event Replay" logic to avoid conflicts.
- **Memory Pressure:** On mobile, a large vector index can consume significant RAM. Memory-mapping (mmap) is required.

---

## 8. Demo Spec: Pluggable Storage Traits

To realize the "Database-Blind" vision, we define a two-tier abstraction: **`StorageProvider`** for relational metadata and **`VectorStore`** for semantic indices.

### 8.1 The StorageProvider Trait (Relational Metadata)
This trait abstracts `sqlx`. It allows the runtime to perform CRUD on nodes, edges, and session state without knowing if it's talking to Postgres or SQLite.

```rust
#[async_trait::async_trait]
pub trait StorageProvider: Send + Sync {
    /// Initialize the database (run migrations)
    async fn initialize(&self) -> Result<(), StorageError>;

    /// Node Operations
    async fn upsert_node(&self, ns: &str, node: Node) -> Result<Uuid, StorageError>;
    async fn get_node(&self, ns: &str, id: Uuid) -> Result<Option<Node>, StorageError>;
    async fn delete_node(&self, ns: &str, id: Uuid) -> Result<(), StorageError>;

    /// Edge Operations
    async fn add_edge(&self, ns: &str, edge: Edge) -> Result<(), StorageError>;
    async fn get_neighbors(&self, ns: &str, id: Uuid, depth: u32) -> Result<Vec<Node>, StorageError>;

    /// Session State (Working Memory)
    async fn state_set(&self, ns: &str, sess: &str, key: &str, val: Value) -> Result<(), StorageError>;
    async fn state_get(&self, ns: &str, sess: &str, key: &str) -> Result<Option<Value>, StorageError>;
}
```

### 8.2 The VectorStore Trait (Semantic Intelligence)
This trait decouples the similarity search from the database. It allows the agent to use `pgvector` in the cloud and `hnsw_rs` on the phone.

```rust
#[async_trait::async_trait]
pub trait VectorStore: Send + Sync {
    /// Add a vector to the index associated with a metadata ID
    async fn insert(&self, ns: &str, id: Uuid, vector: Vec<f32>) -> Result<(), VectorError>;

    /// Search the index for the nearest neighbors
    async fn search(&self, ns: &str, query: Vec<f32>, limit: usize) -> Result<Vec<VectorMatch>, VectorError>;

    /// Delete a vector from the index
    async fn delete(&self, ns: &str, id: Uuid) -> Result<(), VectorError>;

    /// Persist index to disk (mandatory for local HNSW)
    async fn commit(&self) -> Result<(), VectorError>;
}

pub struct VectorMatch {
    pub id: Uuid,           // Links back to Node ID in StorageProvider
    pub distance: f32,
    pub namespace: String,
}
```

### 8.3 Usage Example: The "Hybrid Search"
The `smrti` memory layer uses both traits to provide high-level "Memory" features.

```rust
pub struct MemoryManager {
    storage: Arc<dyn StorageProvider>,
    vectors: Arc<dyn VectorStore>,
}

impl MemoryManager {
    pub async fn search(&self, ns: &str, query_text: &str, limit: usize) -> Result<Vec<Node>, Error> {
        // 1. Generate embedding (using rig/fastembed)
        let embedding = self.embedder.embed(query_text).await?;

        // 2. Perform vector search via the trait (could be pgvector or local HNSW)
        let matches = self.vectors.search(ns, embedding, limit).await?;

        // 3. Fetch full node metadata from SQL via the StorageProvider trait
        let mut results = Vec::new();
        for m in matches {
            if let Some(node) = self.storage.get_node(ns, m.id).await? {
                results.push(node);
            }
        }
        Ok(results)
    }
}
```

### 8.4 Platform Implementations

| Tier | Metadata (`StorageProvider`) | Search (`VectorStore`) |
| :--- | :--- | :--- |
| **Cloud** | `PostgresProvider` (sqlx) | `PgVectorStore` (SQL-based search) |
| **Edge** | `SqliteProvider` (sqlx) | `LocalVectorStore` (hnsw_rs + disk persistence) |

---

## 9. Conclusion

The biggest blocker to Konf's "Billion Dollar" potential is its current hard dependency on Postgres. By abstracting storage and moving toward a **Rust-native Vector engine**, Konf ceases to be a "wrapper" and becomes a **new category of infrastructure**: a secure, portable, local-first OS for the agentic age. 

This architecture allows a homelab owner to run the same "Brain" on their server and their phone, with total data sovereignty and zero trust required of the cloud.
