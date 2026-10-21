# Generic installer for any Linux distribution.
#
#   make                 # build target/release/routwatch
#   make install         # PREFIX=/usr/local by default
#   make install PREFIX=$HOME/.local
#   make test
#
# Distribution packages (Arch: packaging/PKGBUILD) are preferred where they
# exist; this file is the fallback for Ubuntu, Fedora, Alpine and anything else.

CARGO ?= cargo
PREFIX ?= /usr/local
DESTDIR ?=
BINDIR := $(DESTDIR)$(PREFIX)/bin
DOCDIR := $(DESTDIR)$(PREFIX)/share/doc/routwatch
LICDIR := $(DESTDIR)$(PREFIX)/share/licenses/routwatch
BIN := target/release/routwatch

.PHONY: all build test lint fmt install uninstall clean

all: build

build:
	$(CARGO) build --release --locked

test:
	$(CARGO) test --locked

lint:
	$(CARGO) clippy --all-targets --locked -- -D warnings

fmt:
	$(CARGO) fmt --all --check

install: build
	install -Dm755 $(BIN) $(BINDIR)/routwatch
	install -Dm644 README.md $(DOCDIR)/README.md
	install -Dm644 LICENSE $(LICDIR)/LICENSE
	install -Dm644 NOTICE $(LICDIR)/NOTICE

uninstall:
	rm -f $(BINDIR)/routwatch
	rm -rf $(DOCDIR) $(LICDIR)

clean:
	$(CARGO) clean
