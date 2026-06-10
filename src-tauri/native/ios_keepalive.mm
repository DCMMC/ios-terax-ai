// Background keep-alive for the iOS LinuxKit engine.
//
// iOS suspends a backgrounded app within ~30s, which freezes the emulator
// threads (and the debug bridge) — so long-running guest work (claude, builds)
// dies the moment the app loses focus. A terminal/shell app keeps running by
// holding an *active audio session that is playing*. We play continuous silence
// (volume 0) with `mixWithOthers`, so the app retains background execution
// without interrupting the user's music. Same approach iSH uses.
//
// Requires `UIBackgroundModes` to include `audio` in Info.plist (added by
// scripts/patch-ios-project.mjs).

#import <AVFoundation/AVFoundation.h>
#import <Foundation/Foundation.h>

static AVAudioEngine *gEngine;
static AVAudioPlayerNode *gPlayer;

static void terax_keepalive_activate(void) {
  NSError *err = nil;
  AVAudioSession *session = [AVAudioSession sharedInstance];
  [session setCategory:AVAudioSessionCategoryPlayback
           withOptions:AVAudioSessionCategoryOptionMixWithOthers
                 error:&err];
  if (err) NSLog(@"[keepalive] setCategory error: %@", err);
  err = nil;
  [session setActive:YES error:&err];
  if (err) NSLog(@"[keepalive] setActive error: %@", err);

  if (!gEngine.isRunning) {
    err = nil;
    if (![gEngine startAndReturnError:&err]) {
      NSLog(@"[keepalive] engine start error: %@", err);
      return;
    }
  }
  if (!gPlayer.isPlaying) {
    [gPlayer play];
  }
}

extern "C" void TeraxStartBackgroundKeepAlive(void) {
  static dispatch_once_t once;
  dispatch_once(&once, ^{
    AVAudioFormat *fmt = [[AVAudioFormat alloc] initStandardFormatWithSampleRate:44100.0
                                                                       channels:2];
    gEngine = [[AVAudioEngine alloc] init];
    gPlayer = [[AVAudioPlayerNode alloc] init];
    [gEngine attachNode:gPlayer];
    [gEngine connect:gPlayer to:gEngine.mainMixerNode format:fmt];

    // One second of silence (zero-filled), looped forever.
    AVAudioFrameCount frames = (AVAudioFrameCount)fmt.sampleRate;
    AVAudioPCMBuffer *buf = [[AVAudioPCMBuffer alloc] initWithPCMFormat:fmt
                                                         frameCapacity:frames];
    buf.frameLength = frames;  // buffers are zero-initialized => silence

    terax_keepalive_activate();
    [gPlayer scheduleBuffer:buf
                     atTime:nil
                    options:AVAudioPlayerNodeBufferLoops
          completionHandler:nil];
    [gPlayer play];

    // Re-activate after interruptions (calls, route changes) so we don't get
    // permanently suspended once something else grabs/releases audio.
    NSNotificationCenter *nc = [NSNotificationCenter defaultCenter];
    [nc addObserverForName:AVAudioSessionInterruptionNotification
                    object:nil
                     queue:nil
                usingBlock:^(NSNotification *note) {
      NSNumber *type = note.userInfo[AVAudioSessionInterruptionTypeKey];
      if (type.unsignedIntegerValue == AVAudioSessionInterruptionTypeEnded) {
        terax_keepalive_activate();
      }
    }];
    [nc addObserverForName:AVAudioSessionMediaServicesWereResetNotification
                    object:nil
                     queue:nil
                usingBlock:^(NSNotification *note) {
      terax_keepalive_activate();
    }];

    NSLog(@"[keepalive] background audio keep-alive started");
  });
}
