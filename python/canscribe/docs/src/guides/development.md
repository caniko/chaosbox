# Development

Enter the development shell:

```sh
nix develop
```

Run the main checks:

```sh
uv run mypy .
uv run pytest
nix fmt
```

## Documentation

Build the mdBook documentation:

```sh
nix build .#docs
```

Build the Pages site:

```sh
nix build .#site
```

The `site` output is the mdBook build published at the project Pages URL.
