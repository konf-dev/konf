# Konf Products

A **product** is a complete AI agent application defined entirely through configuration — no code required.

A product is a directory containing:

```
my-product/
├── config/
│   ├── konf.toml           # Platform overrides (optional)
│   ├── tools.yaml           # Which tools this product uses
│   ├── models.yaml          # LLM provider and model settings
│   ├── project.yaml         # Product metadata, capabilities, triggers
│   └── workflows/           # Workflow definitions (YAML)
│       └── chat.yaml
├── prompts/                 # System prompts and templates (Markdown)
│   └── system.md
└── README.md
```

## Reference Products

| Product | Description |
|---------|-------------|
| [assistant](assistant/) | Personal assistant with memory, chat, and tool use |

## Creating a New Product

1. Copy `_template/` to a new directory
2. Edit `config/tools.yaml` to select your tools
3. Write workflows in `config/workflows/`
4. Write your system prompt in `prompts/`
5. Run: `KONF_CONFIG_DIR=products/my-product/config cargo run --bin konf-backend`

See [docs/product-guide/creating-a-product.md](../docs/product-guide/creating-a-product.md) for a full walkthrough.
