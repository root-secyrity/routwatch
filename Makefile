# Generic installer for any Linux distribution.
#
#   make                 # build target/release/routwatch
#   make install         # PREFIX=/usr/local by default, plus bash/zsh/fish completions
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
BASHCOMPDIR := $(DESTDIR)$(PREFIX)/share/bash-completion/completions
ZSHCOMPDIR := $(DESTDIR)$(PREFIX)/share/zsh/site-functions
FISHCOMPDIR := $(DESTDIR)$(PREFIX)/share/fish/vendor_completions.d
BIN := target/release/routwatch

.PHONY: all build test lint fmt install uninstall completions clean

all: build

build:
	$(CARGO) build --release --locked

test:
	$(CARGO) test --locked

lint:
	$(CARGO) clippy --all-targets --locked -- -D warnings

fmt:
	$(CARGO) fmt --all --check

# The binary prints its own scripts, so the completions can never drift from
# the flags the build actually accepts.
completions: build
	install -d $(BASHCOMPDIR) $(ZSHCOMPDIR) $(FISHCOMPDIR)
	$(BIN) --completion bash > $(BASHCOMPDIR)/routwatch
	$(BIN) --completion zsh > $(ZSHCOMPDIR)/_routwatch
	$(BIN) --completion fish > $(FISHCOMPDIR)/routwatch.fish
	chmod 644 $(BASHCOMPDIR)/routwatch $(ZSHCOMPDIR)/_routwatch $(FISHCOMPDIR)/routwatch.fish

install: build completions
	install -Dm755 $(BIN) $(BINDIR)/routwatch
	install -Dm644 README.md $(DOCDIR)/README.md
	install -Dm644 LICENSE $(LICDIR)/LICENSE
	install -Dm644 NOTICE $(LICDIR)/NOTICE

uninstall:
	rm -f $(BINDIR)/routwatch
	rm -rf $(DOCDIR) $(LICDIR)
	rm -f $(BASHCOMPDIR)/routwatch $(ZSHCOMPDIR)/_routwatch $(FISHCOMPDIR)/routwatch.fish

clean:
	$(CARGO) clean
