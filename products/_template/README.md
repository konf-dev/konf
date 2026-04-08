# Product Template

Copy this directory to create a new Konf product:

```bash
cp -r products/_template products/my-product
```

Then edit the files to define your product:

1. `config/tools.yaml` — select which tools your product uses
2. `config/workflows/hello.yaml` — replace with your workflows
3. Add a `prompts/` directory for system prompts if needed

Run your product:

```bash
KONF_CONFIG_DIR=products/my-product/config cargo run --bin konf-backend
```
