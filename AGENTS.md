# Repository Guidelines

## Project Structure & Module Organization

GitDock is a Tauri desktop application with a React/TypeScript frontend and a Rust backend.

- `src/`: UI code. `App.tsx` owns the main interface, `api.ts` wraps Tauri commands, `styles.css` contains global styles, and `main.tsx` is the browser entry point.
- `src-tauri/src/`: native application code. Keep Git process logic in `git.rs`, persisted settings in `store.rs`, shared data types in `models.rs`, and Tauri commands/state wiring in `lib.rs`.
- `src/App.test.tsx`: frontend tests; Rust unit tests live beside their implementation in `#[cfg(test)]` modules.
- `src-tauri/icons/`: packaged application icons.
- `dist/` and `src-tauri/target/`: generated build output; root `dist/` keeps the frontend bundle plus the final `.app`, but no `.dmg`. Never commit them.

## Build, Test, and Development Commands

- `npm install`: install the locked Node dependencies. Node 24 or newer is required.
- `npm run dev`: start the Vite frontend only.
- `npm run tauri dev`: run the complete desktop app with hot reload.
- `npm run build`: type-check TypeScript and create the frontend bundle in `dist/`.
- `npm run tauri build`: build the release application under `src-tauri/target/`.
- `npm run package`: build one macOS `.app`, keep the frontend bundle in root `dist/`, and copy the `.app` there without producing a `.dmg`.
- `npm test`: run Vitest once.
- `cargo test --manifest-path src-tauri/Cargo.toml`: run Rust tests.
- `cargo fmt --manifest-path src-tauri/Cargo.toml --check`: verify Rust formatting.

## Coding Style & Naming Conventions

TypeScript is strict. Match the existing two-space indentation, double quotes, semicolons, `camelCase` functions/variables, and `PascalCase` components and interfaces. Keep Tauri calls centralized in `src/api.ts`. Rust follows `rustfmt`: four-space indentation, `snake_case` functions/modules, and `PascalCase` types. Prefer focused changes and existing patterns over new abstractions. No JavaScript lint command is currently configured.

## Testing Guidelines

Use Vitest with Testing Library for user-visible React behavior; name files `*.test.ts` or `*.test.tsx` and colocate them with the source. Add Rust unit tests near the affected module. There is no coverage threshold, but bug fixes should include a regression test. Run both frontend and Rust test commands before submitting.

## Commit & Pull Request Guidelines

Current history uses Conventional Commit-style subjects such as `feat: implement GitDock v1`. Continue with concise, imperative prefixes (`feat:`, `fix:`, `test:`, `docs:`). Pull requests should explain behavior changes, list verification commands, link relevant issues, and include screenshots for UI changes. Call out destructive Git-operation or persistence changes explicitly.

## Documentation Sync

- `README.md` is the Chinese documentation and `README.en.md` is its English counterpart.
- For every product update, review both files and update them together whenever features, behavior, requirements, commands, project structure, or packaging output changes.
- Keep both README versions structurally equivalent and do not merge a documentation-affecting change with only one language updated.

## Security & Configuration

Treat frontend input and repository paths as untrusted at the Tauri boundary. Preserve path validation and confirmation flows for destructive Git operations. Do not commit local repositories, credentials, logs, or generated build directories.
