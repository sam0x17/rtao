"""Record the pinned Typer command/option/environment tree."""

from __future__ import annotations

from enum import Enum
from pathlib import Path

import click
from typer.main import get_command
from bittensor.cli.main import app

from common import jsonable, write


def dump(directory: Path) -> None:
    def command(node):
        options = []
        for parameter in node.params:
            default = parameter.default
            if isinstance(default, Enum):
                default = default.value
            if callable(default):
                default = {"callable": f"{default.__module__}.{default.__qualname__}"}
            options.append({"name": parameter.name, "options": list(parameter.opts),
                            "secondary_options": list(parameter.secondary_opts),
                            "required": parameter.required, "multiple": parameter.multiple,
                            "nargs": parameter.nargs, "envvar": parameter.envvar,
                            "default": jsonable(default), "type": parameter.type.name})
        children = []
        if isinstance(node, click.Group):
            for name, child in sorted(node.commands.items()):
                child_record = command(child)
                child_record["name"] = name
                children.append(child_record)
        return {"name": node.name, "help": node.help, "options": options, "commands": children}
    write(directory / "cli_tree.json", {"schema": 1,
          "source": "sdk/python/bittensor/cli/main.py", "tree": command(get_command(app))})
