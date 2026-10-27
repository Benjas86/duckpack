import argparse
import sys
from . import init, build, apply, diff, compile, explore, __version__, __duckdb_version__

def main():
    parser = argparse.ArgumentParser(
        prog="duckpack",
        description="Declarative schema management, interactive terminal IDE, and in-memory inspection for DuckDB",
    )
    parser.add_argument(
        "-v", "--version",
        action="version",
        version=f"duckpack {__version__} (DuckDB engine: {__duckdb_version__})",
    )
    parser.add_argument(
        "-e", "--env",
        default=None,
        help="Optional environment profile (e.g. dev, prod). Loads .env.<profile>",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    # init
    p_init = subparsers.add_parser("init", help="Initialize a new DuckDB migration project")
    p_init.add_argument("-p", "--project-dir", default=".", help="Project directory (default: .)")

    # build
    p_build = subparsers.add_parser("build", help="Validate project syntax using an in-memory Shadow DB")
    p_build.add_argument("-p", "--project-dir", default=".", help="Project directory (default: .)")

    # apply
    p_apply = subparsers.add_parser("apply", help="Apply migrations to target database")
    p_apply.add_argument("-p", "--project-dir", default=".", help="Project directory (default: .)")
    p_apply.add_argument("-d", "--db", default=None, help="Target database path/URI (defaults to duckpack.toml [target] or local.duckdb)")
    p_apply.add_argument("--database", "--catalog", dest="database", default=None, help="Target database catalog name if multiple databases are attached")
    p_apply.add_argument("--force-drop", action="store_true", help="Allow destructive column/table drops")
    p_apply.add_argument("--interactive", action="store_true", help="Launch interactive Diff & Apply TUI")
    p_apply.add_argument("--auto-approve", action="store_true", help="Apply migrations automatically without interactive review")
    p_apply.add_argument("--quack-token", default=None, help="Authentication token for Quack protocol")
    p_apply.add_argument("--insecure", action="store_true", help="Disable SSL/TLS verification")
    p_apply.add_argument("-e", "--env", default=None, help="Optional environment profile (e.g. dev, prod)")

    # diff
    p_diff = subparsers.add_parser("diff", help="Show schema differences between project and target database")
    p_diff.add_argument("-p", "--project-dir", default=".", help="Project directory (default: .)")
    p_diff.add_argument("-d", "--db", default=None, help="Target database path/URI (defaults to duckpack.toml [target] or local.duckdb)")
    p_diff.add_argument("--database", "--catalog", dest="database", default=None, help="Target database catalog name if multiple databases are attached")
    p_diff.add_argument("--quack-token", default=None, help="Authentication token for Quack protocol")
    p_diff.add_argument("--insecure", action="store_true", help="Disable SSL/TLS verification")
    p_diff.add_argument("-e", "--env", default=None, help="Optional environment profile (e.g. dev, prod)")

    # compile
    p_compile = subparsers.add_parser("compile", help="Compile project into a .duckpack snapshot artifact")
    p_compile.add_argument("-p", "--project-dir", default=".", help="Project directory (default: .)")
    p_compile.add_argument("-o", "--out", required=True, help="Output .duckpack file path")

    # explore
    p_explore = subparsers.add_parser("explore", help="Launch interactive terminal IDE")
    p_explore.add_argument("-p", "--project-dir", default=".", help="Project directory (default: .)")
    p_explore.add_argument("-d", "--db", default=None, help="Target database path/URI (defaults to duckpack.toml [target] or local.duckdb)")
    p_explore.add_argument("--database", "--catalog", dest="database", default=None, help="Target database catalog name if multiple databases are attached")
    p_explore.add_argument("--quack-token", default=None, help="Authentication token for Quack protocol")
    p_explore.add_argument("--insecure", action="store_true", help="Disable SSL/TLS verification")
    p_explore.add_argument("-e", "--env", default=None, help="Optional environment profile (e.g. dev, prod)")

    args = parser.parse_args()
    env_profile = getattr(args, "env", None)

    if args.command == "init":
        init(args.project_dir)
        print(f"Initialized DuckPack project in {args.project_dir}")
    elif args.command == "build":
        build(args.project_dir)
        print("Build successful! All SQL definitions are valid.")
    elif args.command == "apply":
        interactive = args.interactive or (not args.auto_approve)
        try:
            res = apply(
                args.db,
                project_dir=args.project_dir,
                database=args.database,
                auto_approve=args.auto_approve,
                force_drop=args.force_drop,
                interactive=interactive,
                quack_token=args.quack_token,
                insecure=args.insecure,
                env=env_profile,
            )
            if res.success:
                print(f"✓ Applied {res.applied_count} change(s) successfully.")
            else:
                print("Migration cancelled.")
        except Exception as e:
            print(f"❌ Error applying migrations: {e}")
            try:
                input("\nPress Enter to exit...")
            except Exception:
                pass
            sys.exit(1)
    elif args.command == "diff":
        res = diff(
            args.db,
            project_dir=args.project_dir,
            database=args.database,
            quack_token=args.quack_token,
            insecure=args.insecure,
            env=env_profile,
        )
        if res.has_changes:
            print(f"Found {res.change_count} pending change(s):")
            for item in res.items:
                print(f"  - {item}")
        else:
            print("Database is up to date (zero drift).")
    elif args.command == "compile":
        compile(args.project_dir, args.out)
        print(f"Compiled snapshot to {args.out}")
    elif args.command == "explore":
        explore(
            args.db,
            project_dir=args.project_dir,
            database=args.database,
            quack_token=args.quack_token,
            insecure=args.insecure,
            env=env_profile,
        )

if __name__ == "__main__":
    main()
