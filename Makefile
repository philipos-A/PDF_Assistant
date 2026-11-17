PYTHON ?= python3

.PHONY: setup setup-backend setup-frontend dev backend frontend test build

setup: setup-backend setup-frontend ## Install backend and frontend dependencies

setup-backend:
	cd backend && $(PYTHON) -m venv .venv && .venv/bin/pip install -U pip && .venv/bin/pip install -r requirements.txt
	@test -f backend/.env || (cp backend/.env.example backend/.env && echo "Created backend/.env: add your ANTHROPIC_API_KEY")

setup-frontend:
	cd frontend && npm install

dev: ## Run backend and frontend together
	./dev.sh

backend:
	cd backend && .venv/bin/uvicorn app.main:app --reload --port 8000

frontend:
	cd frontend && npm run dev

test: ## Run backend tests
	cd backend && .venv/bin/pytest -q

build: ## Type-check and build the frontend
	cd frontend && npm run build
