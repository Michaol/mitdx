""" mitdx 命令行工具 (Command Line Interface) """
import argparse
import sys
from mitdx.quotes import Quotes
from mitdx.affair import Affair
from mitdx.reader import Reader

def _init_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="mitdx: High-performance TDX data client"
    )
    subparsers = parser.add_subparsers(dest="command", help="Available commands")

    # Command: quotes
    q_parser = subparsers.add_parser("quotes", help="Fetch real-time quotes bars")
    q_parser.add_argument("--symbol", default="600036", help="Stock code (e.g. 600036)")
    q_parser.add_argument("--freq", type=int, default=9, help="Frequency (0=5min..9=day)")
    q_parser.add_argument("--start", type=int, default=0, help="Start offset")
    q_parser.add_argument("--count", type=int, default=10, help="Number of bars to fetch")
    q_parser.add_argument("--backend", default="pandas", choices=["pandas", "polars"], help="DataFrame backend")

    # Command: affair
    a_parser = subparsers.add_parser("affair", help="Fetch or parse financial zip data")
    a_parser.add_argument("action", choices=["list", "fetch", "parse"], help="Action to perform")
    a_parser.add_argument("--filename", default=None, help="Filename (e.g. gpcw.txt or specific zip)")
    a_parser.add_argument("--downdir", default=".", help="Download directory")
    a_parser.add_argument("--backend", default="pandas", choices=["pandas", "polars"], help="DataFrame backend (for parse)")

    # Command: reader
    r_parser = subparsers.add_parser("reader", help="Read offline VIPDOC data")
    r_parser.add_argument("type", choices=["daily", "minute", "fzline"], help="Data type")
    r_parser.add_argument("--symbol", required=True, help="Stock code (e.g. sh600036)")
    r_parser.add_argument("--tdxdir", default="C:/new_tdx", help="TDX installation directory")
    r_parser.add_argument("--market", default="std", choices=["std", "ext"], help="Market type")

    return parser

def _handle_quotes(args):
    with Quotes(market="std") as q:
        df = q.bars(symbol=args.symbol, frequency=args.freq, start=args.start, count=args.count, backend=args.backend)
        print(df)

def _handle_affair(args):
    if args.action == "list":
        for f in Affair.files():
            print(f"{f['filename']}\t{f['filesize']} bytes\t{f['hash']}")
    elif args.action == "fetch":
        if not args.filename:
            print("Error: --filename is required for fetch", file=sys.stderr)
            sys.exit(1)
        Affair.fetch(downdir=args.downdir, filename=args.filename)
        print(f"Successfully fetched {args.filename} to {args.downdir}")
    elif args.action == "parse":
        if not args.filename:
            print("Error: --filename is required for parse", file=sys.stderr)
            sys.exit(1)
        df = Affair.parse(downdir=args.downdir, filename=args.filename, backend=args.backend)
        print(df)

def _handle_reader(args):
    r = Reader.factory(market=args.market, tdxdir=args.tdxdir)
    if args.type == "daily":
        df = r.daily(symbol=args.symbol)
    elif args.type == "minute":
        df = r.minute(symbol=args.symbol, suffix=1)
    elif args.type == "fzline":
        df = r.fzline(symbol=args.symbol)
    else:
        df = None
        
    if df is not None:
        print(df)
    else:
        print(f"No {args.type} data found for {args.symbol} in {args.tdxdir}", file=sys.stderr)

def main():
    parser = _init_parser()
    args = parser.parse_args()

    if not args.command:
        parser.print_help()
        sys.exit(1)

    try:
        if args.command == "quotes":
            _handle_quotes(args)
        elif args.command == "affair":
            _handle_affair(args)
        elif args.command == "reader":
            _handle_reader(args)
    except Exception as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)

if __name__ == "__main__":
    main()
