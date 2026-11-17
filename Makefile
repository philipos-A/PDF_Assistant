.PHONY: setup setup-backend setup-frontend dev backend frontend test lint build

setup: setup-backend setup-frontend ## Fetch/build backend crates and install npm deps

setup-backend:
	cd backend && cargo build --release
	@test -f backend/.env || (cp backend/.env.example backend/.env && echo "Created backend/.env: add your ANTHROPIC_API_KEY")

setup-frontend:
	cd frontend && npm install

dev: ## Run backend and frontend together
	./dev.sh

backend:
	cd backend && cargo run --release

frontend:
	cd frontend && npm run dev

test: ## Run backend tests (Claude is mocked; no API calls)
	cd backend && cargo test

lint:
	cd backend && cargo clippy --all-targets -- -D warnings && cargo fmt --check

build: ## Release-build the backend and production-build the frontend
	cd backend && cargo build --release
	cd frontend && npm run build
