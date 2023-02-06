EXE_linux_x86_64 = ./target/release/tw-nhi-icc-service
EXE_windows_x86_64 = ./target/x86_64-pc-windows-gnu/release/tw-nhi-icc-service.exe
EXE_windows_i686 = ./target/i686-pc-windows-gnu/release/tw-nhi-icc-service.exe
INSTALLED_EXE = /usr/local/bin/tw-nhi-icc-service.exe

all: linux_x86_64 windows_x86_64 windows_i686
	cp "$(EXE_linux_x86_64)" "$$(dirname "$(EXE_linux_x86_64)")/../tw-nhi-icc-service-linux-x86_64"
	cp "$(EXE_windows_x86_64)" "$$(dirname "$(EXE_windows_x86_64)")/../../tw-nhi-icc-service-windows-x86_64.exe"
	cp "$(EXE_windows_i686)" "$$(dirname "$(EXE_windows_i686)")/../../tw-nhi-icc-service-windows-x86.exe"

linux_x86_64: $(EXE_linux_x86_64)

windows_x86_64: $(EXE_windows_x86_64)

windows_i686: $(EXE_windows_i686)

$(EXE_linux_x86_64): $(shell find . -type f -iname '*.rs' -o -name 'Cargo.toml' | grep -v ./target | sed 's/ /\\ /g')
	cargo build --release
	strip $(EXE_linux_x86_64)

$(EXE_windows_x86_64): $(shell find . -type f -iname '*.rs' -o -name 'Cargo.toml' | grep -v ./target | sed 's/ /\\ /g')
	cross build --release --target x86_64-pc-windows-gnu
	strip $(EXE_windows_x86_64)

$(EXE_windows_i686): $(shell find . -type f -iname '*.rs' -o -name 'Cargo.toml' | grep -v ./target | sed 's/ /\\ /g')
	cross build --release --target i686-pc-windows-gnu
	strip $(EXE_windows_i686)

install:
	$(MAKE)
	sudo cp $(EXE_linux) $(INSTALLED_EXE)
	sudo chown root: $(INSTALLED_EXE)
	sudo chmod 0755 $(INSTALLED_EXE)

uninstall:
	sudo rm $(INSTALLED_EXE)

test:
	cargo test --verbose

clean:
	cargo clean