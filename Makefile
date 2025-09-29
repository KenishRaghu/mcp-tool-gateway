.PHONY: test build evals evals-hermes harbor sentinel-test gate gate-bad up

build:
	cd gateway && cargo build --release

test:
	cd gateway && cargo test
	python -m pytest -q evals/tests
	cd infra/sentinel && sentinel test

evals: build                 ## scripted worst-case agent, enforce vs no-protection baseline
	python evals/run_local.py --agent scripted --mode both

evals-hermes: build          ## real Hermes model (HERMES_BASE_URL / HERMES_MODEL / HERMES_API_KEY)
	python evals/run_local.py --agent hermes --mode both

harbor:                      ## run the same tasks in Harbor containers
	docker build -t mcp-gateway:local .
	PYTHONPATH=evals harbor run -p evals/tasks --agent hermes_agent.harbor_agent:HermesHarborAgent --ak agent_kind=scripted

sentinel-test:
	cd infra/sentinel && sentinel test

gate:                        ## terraform plan -> Sentinel (no AWS creds needed)
	MOCK_AWS=1 infra/scripts/gate.sh environments/production.tfvars

gate-bad:                    ## shows the gate blocking an insecure rollout
	MOCK_AWS=1 infra/scripts/gate.sh environments/insecure-example.tfvars

up:
	docker compose up --build
