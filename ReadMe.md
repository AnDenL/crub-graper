## CRUPP
Made to ease c++ development with it's new features like modules.

**Why rust?**

Cause it's sounds funnier

### Instalation

You need [rust](https://rust-lang.org/learn/get-started/)

If you have it, the easiest way is:
```bash
cargo install --path .
```
and (if you want)
```bash
cargo build --release 
sudo cp target/release/crub-graper /usr/local/bin/
```

Also there is an aur package
```bash
yay -S crupp-bin
```

### Usage

Make Crub.toml file in your project it should look like this
```toml
[package]
compiler = "clang++" # or g++ any you want
standard = "-std=c++2b" # c++ standart
source_dir = "./src/" 
out_dir = "./build"

# Compilation flags and libraries, any you need
# flags = []
# include_dirs = []
# lib_dirs = []
# libs = []

[[bin]]
name = "main" # app name
path = "/src/main.cpp" # your main file
```
You can auto generate it with
```bash
crupp toml
```
And
```bash
crupp build
```
or
```bash
crupp run
``` 
Also you can make compilation_commands.json for your language server
```bash
crupp compdb
```

### Is it compatible with C?

Maybe
