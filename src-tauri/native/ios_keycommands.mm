#import <UIKit/UIKit.h>
#import <WebKit/WebKit.h>
#import <objc/runtime.h>

extern "C" void TeraxInstallKeyCommands(void) {}

@implementation WKWebView (TeraxKeyCommands)

+ (void)load {
    Method original = class_getInstanceMethod(self, @selector(keyCommands));
    Method replacement = class_getInstanceMethod(self, @selector(terax_keyCommands));

    if (!replacement) {
        return;
    }

    if (original &&
        class_addMethod(self, @selector(keyCommands), method_getImplementation(replacement),
                        method_getTypeEncoding(replacement))) {
        class_replaceMethod(self, @selector(terax_keyCommands), method_getImplementation(original),
                            method_getTypeEncoding(original));
    } else if (original) {
        method_exchangeImplementations(original, replacement);
    }
}

- (NSArray<UIKeyCommand *> *)terax_keyCommands {
    NSArray<UIKeyCommand *> *commands = [self terax_keyCommands];
    UIKeyCommand *settings = [UIKeyCommand keyCommandWithInput:@","
                                                 modifierFlags:UIKeyModifierCommand
                                                        action:@selector(terax_openSettings:)];
    settings.discoverabilityTitle = @"Open Terax Settings";

    if (commands) {
        return [commands arrayByAddingObject:settings];
    }
    return @[ settings ];
}

- (void)terax_openSettings:(UIKeyCommand *)sender {
    (void)sender;
    [self evaluateJavaScript:@"window.dispatchEvent(new CustomEvent('terax:settings-open',{detail:'general'}));"
           completionHandler:nil];
}

@end
