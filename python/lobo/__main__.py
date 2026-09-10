"""Support ``python -m lobo`` as well as the installed console command."""

from .cli import main

raise SystemExit(main())
