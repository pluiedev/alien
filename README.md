# `alien`

A Rust port of [`alien`](https://sourceforge.net/projects/alien-pkg-convert/),
a tool that converts software packages to work from one package manager to the next.

Currently, the tool supports converting between:
 - `.deb` packages — used by `dpkg`, prevalent in Linux distributions (distros)
   derived from Debian and Ubuntu;
 - `.rpm` packages — used by `rpm`, found in Red Hat-derived distros such as RHEL,
   CentOS, openSUSE, Fedora and more;
 - LSB packages — used by Linux Standard Base and are basicaly `.rpm` packages

More package formats will be added in the future, including three additional formats
available for the original `alien`: 
 - `.tgz` packages — used by Slackware Linux
 - `.pkg` packages — used by Solaris
 - `.slp` packages — used by Stampede Linux

## Motivation

The main goal for this port is to enhance the original `alien`'s performance,
error handling and versatility, which were all hindered by the language `alien` was
originally written in, Perl. With Rust, there are a lot more opportunities for
offering the end user parallel processing, more robust error messages, and potentially
portability to other operating systems.

Code-wise, the original `alien`'s control flow was not entirely clear, and sometimes
the program does duplicate work thanks to the use of implicit but overridable accessors.
In comparison, the Rust version minimizes duplicate work, cleanly seperates source packages
and target packages for better readability and comprehension, and overall the code is just
laid out more explicitly which helps users and developers to better debug problems.

In conclusion, I believe rewriting `alien` in Rust aids users and developers alike, and the
benefit far outweighs the cost of my time ~~and my sanity~~, and so this project was born.

## Known Issues

 - Names need to be mapped from `.rpm` to `.deb` - in particular, `.deb` package
       names cannot contain uppercase letters, whereas `.rpm` packages have no such restriction.
	   
 - Currently dependencies from `.deb` files are not processed, which means `.rpm`
	   packages converted from `.deb` packages may not install correctly.

From the original `alien`'s 8.95 release (which is to my knowledge the latest), on top of which this port is based:

 - Handling postinst script when converting to/from .slp packages.
  
 - Alien needs to handle relocatable conffiles, partially relocatable
  packages, and packages that have multiple parts that relocate
  differently.

 - RPM ghost file support. On conversion, make preinst move file out of the
  way, postinst put it back. Thus emulating the behavior of rpm.

 - Seems slackware packages may now incliude an install/slack-desc
  with a description in it

## License

Licensed under [GPLv2](LICENSE). © Leah "pluie" Chen 2023  