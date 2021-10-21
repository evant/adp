# adp (Android Device Pool)

## What is this?

A tool to run device tests against a pool of devices. It will 'checkout' a device to run your tests against and then
return that device back to the pool when it's complete. This allows you to run multiple sets of tests in parallel
without them stepping on each other's toes.

## Usage

All you need to do is prefix your gradle command with `adp`. It will figure which connected device to run on and set
the `ANDROID_SERIAL` env variable. If there's no available devices it will block until one becomes available.

```shell
./gradlew packageDebugAndroidTest
adp ./gradlew connectedAndroidTest
```

Note: It's good practice to ensure everything is built _before_ you call with `adp`. This way it can be building when
otherwise it would be waiting for a device.

## Use Cases

### Multiple ci builds in parallel on the same build machine

You can have your build machine set up with a set of devices or emulators available. Separate builds will independently
run on a device without conflicting with each other.

### Test sharding

You can easily shard tests across devices by running them in parallel. This even works if you have more shards than
available devices, it will just wait until one of them frees up.

```shell
seq 0 1 | xargs -I{} -n 1 -P 2 adp ./gradlew connectedAndroidTest -Pandroid.testInstrumentationRunnerArguments.numShards=2 -Pandroid.testInstrumentationRunnerArguments.shardIndex={}
```

### Easy device boot waiting

Even if you are running a single test run against a single device you can use `adp` to wait until the device is actually
brought up.

```shell
./start-emulator.sh & adp ./gradlew connectedAndroidTest
```

## Limitations

- Right now there's no way to configure how `adp` runs. Additional options like more verbose
logging, specifying adb's path, and grouping devices into 'buckets' are planned.
- All tests are expected to run on the same machine and must all be prefixed with `adp`, otherwise it won't be aware 
that the device is in use.