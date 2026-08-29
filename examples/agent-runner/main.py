from sandkiln import Sandbox

# Stands in for code an LLM agent would generate at runtime — hardcoded
# here because this is a reference example, not a live agent. A real
# runner substitutes whatever script the agent produced for this string.
AGENT_SCRIPT = """
import json

data = {"answer": 2 + 2, "note": "written by the agent-generated script"}
with open("/tmp/result.json", "w") as f:
    json.dump(data, f)

print("agent script finished")
"""


def main() -> None:
    print("Creating sandbox...")
    sandbox = Sandbox.create(tags={"example": "agent-runner"})
    print(f"Sandbox {sandbox.id} ready.")

    try:
        sandbox.write_file("/tmp/agent_task.py", AGENT_SCRIPT)

        result = sandbox.run_command("python3", ["/tmp/agent_task.py"])
        print("--- stdout ---")
        print(result.stdout)
        print("--- stderr ---")
        print(result.stderr)
        print(f"--- exit code: {result.exit_code} ---")

        if result.exit_code != 0:
            raise SystemExit(result.exit_code)

        result_bytes = sandbox.read_file("/tmp/result.json")
        print("--- result.json ---")
        print(result_bytes.decode("utf-8"))
    finally:
        sandbox.stop()
        print(f"Sandbox {sandbox.id} stopped.")


if __name__ == "__main__":
    main()
